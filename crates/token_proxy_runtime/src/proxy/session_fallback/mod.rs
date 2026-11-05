//! Session fallback：无标记/无会话头时生成 ID，并用首轮 user+assistant 指纹绑定。
//!
//! 设计要点：
//! - 内存最多 50 条 LRU；内存未满时未命中不读文件；内存满且未命中才查文件。
//! - 新绑定立即落盘；`last_used_at` 30 秒 debounce；1 小时 TTL 清理。
//! - pending 以 `(request_id, upstream_id)` 为键，重试复用同一 ID。
//! - 只存哈希与 ID，不存对话原文。

pub(crate) mod extract;
pub(crate) mod stream;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::{interval, Duration, Instant, MissedTickBehavior};

use extract::binding_hash;

pub(crate) const FALLBACK_FILE_NAME: &str = "header_injection_fallback.json";
const MEMORY_LIMIT: usize = 50;
const PENDING_LIMIT: usize = 50;
const FILE_LIMIT: usize = 1000;
const TTL: Duration = Duration::from_secs(60 * 60);
const TTL_MS: u64 = 60 * 60 * 1000;
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

    pub(crate) fn resolve(
        &self,
        upstream_id: &str,
        request_id: &str,
        user_hash: &str,
        assistant_hash: Option<&str>,
    ) -> FallbackResolution {
        let pending_key = (request_id.to_string(), upstream_id.to_string());
        if let Some(pending) = self
            .inner
            .pending
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(&pending_key)
        {
            return FallbackResolution::Pending {
                id: pending.id.clone(),
                capture: self.capture(request_id, upstream_id),
            };
        }

        let Some(assistant_hash) = assistant_hash else {
            let id = generate_id();
            self.insert_pending(
                pending_key,
                Pending {
                    id: id.clone(),
                    user_hash: user_hash.to_string(),
                    created_at: Instant::now(),
                },
            );
            return FallbackResolution::Pending {
                id,
                capture: self.capture(request_id, upstream_id),
            };
        };

        let key = Key {
            upstream_id: upstream_id.to_string(),
            binding_hash: binding_hash(user_hash, assistant_hash),
        };
        if let Some(id) = self.memory_hit(&key) {
            return FallbackResolution::Existing(id);
        }

        let memory_len = self
            .inner
            .memory
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .len();
        if memory_len < MEMORY_LIMIT {
            let id = generate_id();
            self.insert_entry(self.new_entry(upstream_id, key.binding_hash.clone(), id.clone()));
            return FallbackResolution::Existing(id);
        }

        if let Some(entry) = self.read_file_entry(&key) {
            let id = entry.id.clone();
            self.insert_entry(entry);
            return FallbackResolution::Existing(id);
        }

        let id = generate_id();
        self.insert_entry(self.new_entry(upstream_id, key.binding_hash.clone(), id.clone()));
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
        let key = Key {
            upstream_id: upstream_id.to_string(),
            binding_hash: hash.clone(),
        };
        let entry = self.new_entry(upstream_id, hash, pending.id.clone());
        self.insert_entry(entry);
        tracing::debug!(
            upstream_id,
            id = pending.id,
            hash_prefix = %hash_prefix(&key.binding_hash),
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
            self.persist_entries(&due);
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

    fn new_entry(&self, upstream_id: &str, hash: String, id: String) -> Entry {
        let now = now_ms();
        Entry {
            upstream_id: upstream_id.to_string(),
            hash,
            id,
            created_at: now,
            last_used_at: now,
        }
    }

    fn memory_hit(&self, key: &Key) -> Option<String> {
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
        Some(entry.entry.id.clone())
    }

    fn insert_entry(&self, mut entry: Entry) {
        entry.last_used_at = now_ms();
        let key = Key {
            upstream_id: entry.upstream_id.clone(),
            binding_hash: entry.hash.clone(),
        };
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
        self.persist_entries(&to_persist);
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

    fn persist_entries(&self, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let _guard = self.inner.io.lock().unwrap_or_else(|err| err.into_inner());
        let mut merged: HashMap<(String, String), Entry> = read_file_entries(&self.inner.file)
            .into_iter()
            .map(|entry| ((entry.upstream_id.clone(), entry.hash.clone()), entry))
            .collect();
        for entry in entries {
            let key = (entry.upstream_id.clone(), entry.hash.clone());
            match merged.get(&key) {
                Some(existing) if existing.last_used_at > entry.last_used_at => {}
                _ => {
                    merged.insert(key, entry.clone());
                }
            }
        }
        let mut all: Vec<Entry> = merged.into_values().collect();
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
