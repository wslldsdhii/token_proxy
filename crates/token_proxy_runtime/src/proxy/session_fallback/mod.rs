//! Session fallback：无标记/无会话头时生成 ID，并用首轮 user+assistant 指纹绑定。
//!
//! 设计要点：
//! - 首条 user 使用最后一个 `<user_input>` 作为稳定锚点；哈希前剔除全部空白/零宽字符。
//! - assistant 分 Absent / Present / PresentEmpty 三态；PresentEmpty 使用降级绑定，不再每轮新建。
//! - 查找顺序：request pending → 精确 binding → 旧 binding → user 锚点唯一命中 →
//!   user 锚点 pending 补绑定 → 新建。
//! - 内存最多 50 条 LRU；内存未满时未命中不读文件；内存满且未命中才查文件。
//! - 同一 `(upstream_id, id)` 只保留一条记录；新绑定立即落盘；30 秒 debounce；7 天 TTL。

pub(crate) mod extract;
pub(crate) mod stream;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::{interval, Duration, Instant, MissedTickBehavior};

use extract::{binding_hash, SessionFingerprint};

pub(crate) const FALLBACK_FILE_NAME: &str = "header_injection_fallback.json";
const MEMORY_LIMIT: usize = 50;
const PENDING_LIMIT: usize = 50;
const FILE_LIMIT: usize = 1000;
/// 映射保留 7 天；1 小时对真实编码会话太短，中途暂停会换 ID。
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const DEBOUNCE: Duration = Duration::from_secs(30);
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct SessionFallbackStore {
    inner: Arc<Inner>,
}

struct Inner {
    file: PathBuf,
    memory: RwLock<HashMap<Key, MemoryEntry>>,
    pending: RwLock<HashMap<PendingKey, Pending>>,
    io: Mutex<()>,
    tasks_started: AtomicBool,
    touch_seq: AtomicU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    upstream_id: String,
    binding_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    upstream_id: String,
    hash: String,
    /// 首条 user 锚点哈希；旧文件缺省为空字符串。
    #[serde(default)]
    user_hash: String,
    id: String,
    created_at: u64,
    last_used_at: u64,
}

#[derive(Clone, Debug)]
struct MemoryEntry {
    entry: Entry,
    last_activity: Instant,
    dirty: bool,
    touch_seq: u64,
}

#[derive(Clone, Debug)]
struct Pending {
    id: String,
    /// 首条 user 锚点哈希；响应提交时与新 assistant 哈希合成 binding。
    user_hash: String,
    created_at: Instant,
}

type PendingKey = (String, String);

#[derive(Clone)]
pub(crate) enum FallbackResolution {
    Existing(String),
    Pending {
        id: String,
        capture: SessionFallbackCapture,
    },
}

/// 响应侧提交句柄；仅在首轮 pending 时随请求携带。
#[derive(Clone)]
pub(crate) struct SessionFallbackCapture {
    store: SessionFallbackStore,
    request_id: String,
    upstream_id: String,
}

impl SessionFallbackCapture {
    pub(crate) fn upstream_id(&self) -> &str {
        &self.upstream_id
    }

    pub(crate) fn commit(&self, assistant_hash: &str) {
        self.store
            .commit(&self.request_id, &self.upstream_id, assistant_hash);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FilePayload {
    #[serde(default = "file_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

fn file_version() -> u32 {
    1
}

impl SessionFallbackStore {
    pub(crate) fn new(data_dir: &Path) -> Arc<Self> {
        let store = Arc::new(Self {
            inner: Arc::new(Inner {
                file: data_dir.join(FALLBACK_FILE_NAME),
                memory: RwLock::new(HashMap::new()),
                pending: RwLock::new(HashMap::new()),
                io: Mutex::new(()),
                tasks_started: AtomicBool::new(false),
                touch_seq: AtomicU64::new(1),
            }),
        });
        store.load_from_file();
        store.start_background_tasks();
        store
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(data_dir: &Path) -> Arc<Self> {
        let store = Arc::new(Self {
            inner: Arc::new(Inner {
                file: data_dir.join(FALLBACK_FILE_NAME),
                memory: RwLock::new(HashMap::new()),
                pending: RwLock::new(HashMap::new()),
                io: Mutex::new(()),
                tasks_started: AtomicBool::new(false),
                touch_seq: AtomicU64::new(1),
            }),
        });
        store.load_from_file();
        store
    }

    #[cfg(test)]
    pub(crate) fn file_path(&self) -> &Path {
        &self.inner.file
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.inner
            .pending
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .len()
    }

    #[cfg(test)]
    pub(crate) fn file_entry_count(&self) -> usize {
        read_file_entries(&self.inner.file).len()
    }

    pub(crate) fn resolve(
        &self,
        upstream_id: &str,
        request_id: &str,
        fingerprint: &SessionFingerprint,
    ) -> FallbackResolution {
        let pending_key = (request_id.to_string(), upstream_id.to_string());
        if let Some(pending) = self
            .inner
            .pending
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(&pending_key)
        {
            tracing::debug!(
                upstream_id,
                id = pending.id,
                match_kind = "request_pending",
                "session fallback resolved"
            );
            return FallbackResolution::Pending {
                id: pending.id.clone(),
                capture: self.capture(request_id, upstream_id),
            };
        }

        // Absent：真正的首轮，没有可匹配的 assistant 指纹。
        let Some(binding_hash) = fingerprint.binding_hash.as_deref() else {
            let id = generate_id();
            self.insert_pending(
                pending_key,
                Pending {
                    id: id.clone(),
                    user_hash: fingerprint.user_anchor_hash.clone(),
                    created_at: Instant::now(),
                },
            );
            tracing::debug!(
                upstream_id,
                id,
                match_kind = "new_pending",
                "session fallback allocated pending"
            );
            return FallbackResolution::Pending {
                id,
                capture: self.capture(request_id, upstream_id),
            };
        };

        if let Some(entry) = self.find_by_binding(upstream_id, binding_hash) {
            tracing::debug!(
                upstream_id,
                id = entry.id,
                match_kind = "binding",
                hash_prefix = %hash_prefix(&entry.hash),
                "session fallback resolved"
            );
            return FallbackResolution::Existing(entry.id);
        }

        if let Some(legacy_hash) = fingerprint.legacy_binding_hash.as_deref() {
            if let Some(entry) = self.find_by_binding(upstream_id, legacy_hash) {
                let id = entry.id.clone();
                self.rekey_entry(entry, binding_hash, &fingerprint.user_anchor_hash);
                tracing::debug!(
                    upstream_id,
                    id,
                    match_kind = "legacy_binding",
                    hash_prefix = %hash_prefix(binding_hash),
                    "session fallback rekeyed legacy binding"
                );
                return FallbackResolution::Existing(id);
            }
        }

        if let Some(entry) =
            self.find_by_user_anchor_unique(upstream_id, &fingerprint.user_anchor_hash)
        {
            let id = entry.id.clone();
            self.rekey_entry(entry, binding_hash, &fingerprint.user_anchor_hash);
            tracing::debug!(
                upstream_id,
                id,
                match_kind = "user_anchor",
                degraded = fingerprint.degraded,
                hash_prefix = %hash_prefix(binding_hash),
                "session fallback reused by user anchor"
            );
            return FallbackResolution::Existing(id);
        }

        if fingerprint.has_assistant_message {
            if let Some(pending) =
                self.take_pending_by_user_anchor(upstream_id, &fingerprint.user_anchor_hash)
            {
                let entry = self.new_entry(
                    upstream_id,
                    binding_hash.to_string(),
                    fingerprint.user_anchor_hash.clone(),
                    pending.id.clone(),
                );
                self.insert_entry(entry);
                tracing::debug!(
                    upstream_id,
                    id = pending.id,
                    match_kind = "anchor_pending",
                    "session fallback bound pending from history"
                );
                return FallbackResolution::Existing(pending.id);
            }
        }

        let id = generate_id();
        let entry = self.new_entry(
            upstream_id,
            binding_hash.to_string(),
            fingerprint.user_anchor_hash.clone(),
            id.clone(),
        );
        self.insert_entry(entry);
        tracing::debug!(
            upstream_id,
            id,
            match_kind = "new_binding",
            degraded = fingerprint.degraded,
            hash_prefix = %hash_prefix(binding_hash),
            "session fallback created new binding"
        );
        FallbackResolution::Existing(id)
    }

    fn commit(&self, request_id: &str, upstream_id: &str, assistant_hash: &str) {
        let pending_key = (request_id.to_string(), upstream_id.to_string());
        let pending = self
            .inner
            .pending
            .write()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&pending_key);
        let Some(pending) = pending else {
            return;
        };
        let hash = binding_hash(&pending.user_hash, assistant_hash);
        let entry = self.new_entry(
            upstream_id,
            hash.clone(),
            pending.user_hash.clone(),
            pending.id.clone(),
        );
        self.insert_entry(entry);
        tracing::debug!(
            upstream_id,
            id = pending.id,
            hash_prefix = %hash_prefix(&hash),
            "session fallback bound first exchange"
        );
    }

    pub(crate) fn flush_due(&self) {
        self.flush_due_at(Instant::now());
    }

    pub(crate) fn flush_due_at(&self, now: Instant) {
        let mut due = Vec::new();
        {
            let mut memory = self
                .inner
                .memory
                .write()
                .unwrap_or_else(|err| err.into_inner());
            for entry in memory.values_mut() {
                if entry.dirty && now.saturating_duration_since(entry.last_activity) >= DEBOUNCE {
                    entry.dirty = false;
                    due.push(entry.entry.clone());
                }
            }
        }
        if !due.is_empty() {
            self.persist_entries(&due, &[]);
        }
    }

    pub(crate) fn cleanup_expired(&self) {
        self.cleanup_expired_at(Instant::now());
    }

    pub(crate) fn cleanup_expired_at(&self, now: Instant) {
        let now_ms = now_ms();
        {
            let mut memory = self
                .inner
                .memory
                .write()
                .unwrap_or_else(|err| err.into_inner());
            memory.retain(|_, entry| now_ms.saturating_sub(entry.entry.last_used_at) < TTL_MS);
        }
        {
            let mut pending = self
                .inner
                .pending
                .write()
                .unwrap_or_else(|err| err.into_inner());
            pending.retain(|_, entry| now.saturating_duration_since(entry.created_at) < TTL);
        }

        let _guard = self.inner.io.lock().unwrap_or_else(|err| err.into_inner());
        let mut entries = read_file_entries(&self.inner.file);
        let before = entries.len();
        entries.retain(|entry| now_ms.saturating_sub(entry.last_used_at) < TTL_MS);
        entries = dedupe_entries_by_id(entries);
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at));
        entries.truncate(FILE_LIMIT);
        if entries.len() != before {
            write_file_entries(&self.inner.file, &entries);
        }
    }

    fn capture(&self, request_id: &str, upstream_id: &str) -> SessionFallbackCapture {
        SessionFallbackCapture {
            store: self.clone(),
            request_id: request_id.to_string(),
            upstream_id: upstream_id.to_string(),
        }
    }

    fn new_entry(&self, upstream_id: &str, hash: String, user_hash: String, id: String) -> Entry {
        let now = now_ms();
        Entry {
            upstream_id: upstream_id.to_string(),
            hash,
            user_hash,
            id,
            created_at: now,
            last_used_at: now,
        }
    }

    fn memory_len(&self) -> usize {
        self.inner
            .memory
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .len()
    }

    fn touch_memory_key(&self, key: &Key) -> Option<Entry> {
        let now = now_ms();
        let mut memory = self
            .inner
            .memory
            .write()
            .unwrap_or_else(|err| err.into_inner());
        let entry = memory.get_mut(key)?;
        entry.entry.last_used_at = now;
        entry.last_activity = Instant::now();
        entry.dirty = true;
        entry.touch_seq = self.inner.touch_seq.fetch_add(1, Ordering::Relaxed);
        Some(entry.entry.clone())
    }

    fn find_by_binding(&self, upstream_id: &str, binding_hash: &str) -> Option<Entry> {
        let key = Key {
            upstream_id: upstream_id.to_string(),
            binding_hash: binding_hash.to_string(),
        };
        if let Some(entry) = self.touch_memory_key(&key) {
            return Some(entry);
        }
        if self.memory_len() < MEMORY_LIMIT {
            return None;
        }
        let entry = self.read_file_entry(&key)?;
        self.insert_entry(entry.clone());
        Some(entry)
    }

    /// 同一 user 锚点只对应一个 ID 时返回该条；多个 ID 视为歧义。
    fn find_by_user_anchor_unique(&self, upstream_id: &str, user_hash: &str) -> Option<Entry> {
        let mut candidates: Vec<(Option<Key>, Entry)> = Vec::new();
        {
            let memory = self
                .inner
                .memory
                .read()
                .unwrap_or_else(|err| err.into_inner());
            for (key, value) in memory.iter() {
                if key.upstream_id == upstream_id && value.entry.user_hash == user_hash {
                    candidates.push((Some(key.clone()), value.entry.clone()));
                }
            }
        }
        if self.memory_len() >= MEMORY_LIMIT {
            for entry in self.file_entries_by_user(upstream_id, user_hash) {
                if !candidates.iter().any(|(_, item)| item.id == entry.id) {
                    candidates.push((None, entry));
                }
            }
        }

        let ids: HashSet<String> = candidates.iter().map(|(_, entry)| entry.id.clone()).collect();
        if ids.len() != 1 {
            return None;
        }
        let id = ids.into_iter().next()?;
        if let Some((Some(key), _)) = candidates.iter().find(|(_, entry)| entry.id == id) {
            return self.touch_memory_key(key);
        }
        let entry = candidates
            .into_iter()
            .find(|(_, entry)| entry.id == id)
            .map(|(_, entry)| entry)?;
        self.insert_entry(entry.clone());
        Some(entry)
    }

    fn take_pending_by_user_anchor(&self, upstream_id: &str, user_hash: &str) -> Option<Pending> {
        let mut pending = self
            .inner
            .pending
            .write()
            .unwrap_or_else(|err| err.into_inner());
        let matches: Vec<PendingKey> = pending
            .iter()
            .filter(|((_, upstream), value)| {
                upstream == upstream_id && value.user_hash == user_hash
            })
            .map(|(key, _)| key.clone())
            .collect();
        if matches.len() != 1 {
            return None;
        }
        pending.remove(&matches[0])
    }

    fn file_entries_by_user(&self, upstream_id: &str, user_hash: &str) -> Vec<Entry> {
        let _guard = self.inner.io.lock().unwrap_or_else(|err| err.into_inner());
        read_file_entries(&self.inner.file)
            .into_iter()
            .filter(|entry| {
                entry.upstream_id == upstream_id
                    && !entry.user_hash.is_empty()
                    && entry.user_hash == user_hash
            })
            .collect()
    }

    /// 插入或更新一条映射；同一 ID 在内存中已存在时 rekey，而不是新增记录。
    fn insert_entry(&self, mut entry: Entry) {
        entry.last_used_at = now_ms();
        let key = Key {
            upstream_id: entry.upstream_id.clone(),
            binding_hash: entry.hash.clone(),
        };

        let existing = {
            let memory = self
                .inner
                .memory
                .read()
                .unwrap_or_else(|err| err.into_inner());
            memory
                .iter()
                .find(|(key, value)| {
                    key.upstream_id == entry.upstream_id && value.entry.id == entry.id
                })
                .map(|(key, value)| (key.clone(), value.entry.hash.clone()))
        };

        match existing {
            Some((old_key, old_hash)) if old_hash == entry.hash => {
                let mut memory = self
                    .inner
                    .memory
                    .write()
                    .unwrap_or_else(|err| err.into_inner());
                if let Some(value) = memory.get_mut(&old_key) {
                    value.entry.last_used_at = entry.last_used_at;
                    value.entry.user_hash = entry.user_hash;
                    value.last_activity = Instant::now();
                    value.dirty = true;
                    value.touch_seq = self.inner.touch_seq.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
            Some((old_key, old_hash)) => {
                let old_upstream = old_key.upstream_id.clone();
                let mut memory = self
                    .inner
                    .memory
                    .write()
                    .unwrap_or_else(|err| err.into_inner());
                memory.remove(&old_key);
                memory.insert(
                    key,
                    MemoryEntry {
                        entry: entry.clone(),
                        last_activity: Instant::now(),
                        dirty: true,
                        touch_seq: self.inner.touch_seq.fetch_add(1, Ordering::Relaxed),
                    },
                );
                drop(memory);
                self.persist_entries(&[entry], &[(old_upstream, old_hash)]);
                return;
            }
            None => {}
        }

        let mut to_persist = vec![entry.clone()];
        {
            let mut memory = self
                .inner
                .memory
                .write()
                .unwrap_or_else(|err| err.into_inner());
            memory.insert(
                key,
                MemoryEntry {
                    entry,
                    last_activity: Instant::now(),
                    dirty: true,
                    touch_seq: self.inner.touch_seq.fetch_add(1, Ordering::Relaxed),
                },
            );
            while memory.len() > MEMORY_LIMIT {
                let Some(oldest) = memory
                    .iter()
                    .min_by_key(|(_, value)| (value.entry.last_used_at, value.touch_seq))
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some(removed) = memory.remove(&oldest) {
                    if removed.dirty {
                        to_persist.push(removed.entry);
                    }
                }
            }
        }
        self.persist_entries(&to_persist, &[]);
    }

    /// 把既有 entry 从旧 binding 迁移到新 binding（同一 ID）。
    fn rekey_entry(&self, entry: Entry, new_hash: &str, user_hash: &str) {
        let old_key = Key {
            upstream_id: entry.upstream_id.clone(),
            binding_hash: entry.hash.clone(),
        };
        let new_key = Key {
            upstream_id: entry.upstream_id.clone(),
            binding_hash: new_hash.to_string(),
        };
        let mut updated = entry;
        updated.hash = new_hash.to_string();
        updated.user_hash = user_hash.to_string();
        updated.last_used_at = now_ms();

        {
            let mut memory = self
                .inner
                .memory
                .write()
                .unwrap_or_else(|err| err.into_inner());
            memory.remove(&old_key);
            memory.insert(
                new_key,
                MemoryEntry {
                    entry: updated.clone(),
                    last_activity: Instant::now(),
                    dirty: true,
                    touch_seq: self.inner.touch_seq.fetch_add(1, Ordering::Relaxed),
                },
            );
        }
        self.persist_entries(&[updated], &[(old_key.upstream_id, old_key.binding_hash)]);
    }

    fn insert_pending(&self, key: PendingKey, pending: Pending) {
        let mut map = self
            .inner
            .pending
            .write()
            .unwrap_or_else(|err| err.into_inner());
        while map.len() >= PENDING_LIMIT {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(key, pending);
    }

    fn persist_entries(&self, entries: &[Entry], removals: &[(String, String)]) {
        if entries.is_empty() && removals.is_empty() {
            return;
        }
        let _guard = self.inner.io.lock().unwrap_or_else(|err| err.into_inner());
        let mut merged: HashMap<(String, String), Entry> = read_file_entries(&self.inner.file)
            .into_iter()
            .map(|entry| ((entry.upstream_id.clone(), entry.hash.clone()), entry))
            .collect();
        for (upstream_id, hash) in removals {
            merged.remove(&(upstream_id.clone(), hash.clone()));
        }
        for entry in entries {
            let key = (entry.upstream_id.clone(), entry.hash.clone());
            match merged.get(&key) {
                Some(existing) if existing.last_used_at > entry.last_used_at => {}
                _ => {
                    merged.insert(key, entry.clone());
                }
            }
        }
        let mut all = dedupe_entries_by_id(merged.into_values().collect());
        all.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at));
        all.truncate(FILE_LIMIT);
        write_file_entries(&self.inner.file, &all);
    }

    fn read_file_entry(&self, key: &Key) -> Option<Entry> {
        let _guard = self.inner.io.lock().unwrap_or_else(|err| err.into_inner());
        read_file_entries(&self.inner.file)
            .into_iter()
            .filter(|entry| entry.upstream_id == key.upstream_id && entry.hash == key.binding_hash)
            .max_by_key(|entry| entry.last_used_at)
    }

    fn load_from_file(&self) {
        let mut entries = read_file_entries(&self.inner.file);
        let now_ms = now_ms();
        let before = entries.len();
        entries.retain(|entry| now_ms.saturating_sub(entry.last_used_at) < TTL_MS);
        entries = dedupe_entries_by_id(entries);
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at));
        entries.truncate(FILE_LIMIT);
        if entries.len() != before {
            write_file_entries(&self.inner.file, &entries);
        }
        let mut memory = self
            .inner
            .memory
            .write()
            .unwrap_or_else(|err| err.into_inner());
        for entry in entries.into_iter().take(MEMORY_LIMIT) {
            let key = Key {
                upstream_id: entry.upstream_id.clone(),
                binding_hash: entry.hash.clone(),
            };
            memory.insert(
                key,
                MemoryEntry {
                    entry,
                    last_activity: Instant::now(),
                    dirty: false,
                    touch_seq: self.inner.touch_seq.fetch_add(1, Ordering::Relaxed),
                },
            );
        }
    }

    fn start_background_tasks(self: &Arc<Self>) {
        if self.inner.tasks_started.load(Ordering::SeqCst) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .inner
            .tasks_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        handle.spawn(async move {
            let mut flush = interval(FLUSH_INTERVAL);
            flush.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut cleanup = interval(CLEANUP_INTERVAL);
            cleanup.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = flush.tick() => {
                        let Some(inner) = weak.upgrade() else { break; };
                        SessionFallbackStore { inner }.flush_due();
                    }
                    _ = cleanup.tick() => {
                        let Some(inner) = weak.upgrade() else { break; };
                        SessionFallbackStore { inner }.cleanup_expired();
                    }
                }
            }
        });
    }
}

/// 同一 `(upstream_id, id)` 只保留 `last_used_at` 最新的一条。
fn dedupe_entries_by_id(entries: Vec<Entry>) -> Vec<Entry> {
    let mut by_id: HashMap<(String, String), Entry> = HashMap::new();
    for entry in entries {
        let key = (entry.upstream_id.clone(), entry.id.clone());
        match by_id.get(&key) {
            Some(existing) if existing.last_used_at >= entry.last_used_at => {}
            _ => {
                by_id.insert(key, entry);
            }
        }
    }
    by_id.into_values().collect()
}

fn read_file_entries(path: &Path) -> Vec<Entry> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<FilePayload>(&bytes) {
        Ok(payload) if payload.version == 1 => payload.entries,
        Ok(payload) => {
            tracing::warn!(
                path = %path.display(),
                version = payload.version,
                "session fallback map ignored: unsupported version"
            );
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "session fallback map ignored: invalid JSON"
            );
            Vec::new()
        }
    }
}

fn write_file_entries(path: &Path, entries: &[Entry]) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = fs::create_dir_all(parent) {
        tracing::warn!(path = %parent.display(), %error, "session fallback map mkdir failed");
        return;
    }
    let payload = FilePayload {
        version: 1,
        entries: entries.to_vec(),
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&payload) else {
        return;
    };
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), seq));
    if let Err(error) = fs::write(&temp, &bytes) {
        tracing::warn!(path = %temp.display(), %error, "session fallback map temp write failed");
        let _ = fs::remove_file(&temp);
        return;
    }
    if let Err(error) = fs::rename(&temp, path) {
        tracing::warn!(path = %path.display(), %error, "session fallback map rename failed");
        let _ = fs::remove_file(&temp);
    }
}

fn generate_id() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let mut id = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut id, "{byte:02x}");
    }
    id
}

fn hash_prefix(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;