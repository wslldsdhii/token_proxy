use super::extract::{
    assistant_hash, binding_hash, canonical_assistant_from_json_response, canonical_assistant_message,
    extract_first_exchange, legacy_assistant_hash, legacy_user_hash, user_hash, FirstAssistant,
    SessionFingerprint,
};
use super::stream::AssistantAccumulator;
use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "tp-session-fallback-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn body(messages: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": messages })).expect("body")
}

fn fingerprint(user: &str, assistant: Option<&str>) -> SessionFingerprint {
    let messages = match assistant {
        Some(text) => json!([
            { "role": "user", "content": user },
            { "role": "assistant", "content": text }
        ]),
        None => json!([{ "role": "user", "content": user }]),
    };
    extract_first_exchange(&body(messages))
        .expect("exchange")
        .fingerprint()
}

fn empty_assistant_fingerprint(user: &str) -> SessionFingerprint {
    let messages = json!([
        { "role": "user", "content": user },
        { "role": "assistant", "content": null, "reasoning_content": "hidden" }
    ]);
    extract_first_exchange(&body(messages))
        .expect("exchange")
        .fingerprint()
}

#[test]
fn extracts_first_user_and_assistant() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "system", "content": "sys" },
        { "role": "user", "content": "hi" },
        { "role": "assistant", "content": "hello" },
        { "role": "user", "content": "again" }
    ])))
    .expect("exchange");
    assert_eq!(exchange.user_anchor, "hi");
    assert_eq!(exchange.assistant, FirstAssistant::Present("hello".into()));
}

#[test]
fn normalizes_crlf_and_array_content() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "user", "content": [
            { "type": "text", "text": "  hi\r\nthere  " },
            { "type": "input_text", "text": "again" }
        ]},
        { "role": "assistant", "content": [
            { "type": "text", "text": "ok\r\n" }
        ]}
    ])))
    .expect("exchange");
    assert_eq!(exchange.user_anchor, "hithereagain");
    assert_eq!(exchange.assistant, FirstAssistant::Present("ok".into()));
}

#[test]
fn uses_tool_calls_when_assistant_text_is_empty() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "user", "content": "run" },
        { "role": "assistant", "content": null, "tool_calls": [
            { "id": "call_1", "type": "function",
              "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" } }
        ]}
    ])))
    .expect("exchange");
    assert_eq!(exchange.user_anchor, "run");
    let FirstAssistant::Present(assistant) = exchange.assistant else {
        panic!("expected tool calls");
    };
    assert!(assistant.contains("call_1"));
    assert!(assistant.contains("bash"));
    assert!(assistant.contains("{\\\"cmd\\\":\\\"ls\\\"}"));
}

#[test]
fn reasoning_only_assistant_is_present_empty() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "user", "content": "think" },
        { "role": "assistant", "content": null, "reasoning_content": "hidden" }
    ])))
    .expect("exchange");
    assert_eq!(exchange.user_anchor, "think");
    assert_eq!(exchange.assistant, FirstAssistant::PresentEmpty);
}

#[test]
fn stream_accumulator_matches_non_stream_text() {
    let mut accumulator = AssistantAccumulator::default();
    accumulator.consume_data(r#"{"choices":[{"delta":{"content":"hel"}}]}"#);
    accumulator.consume_data(r#"{"choices":[{"delta":{"content":"lo"}}]}"#);
    accumulator.consume_data("[DONE]");
    assert_eq!(accumulator.finish().as_deref(), Some("hello"));

    let response = json!({ "choices": [{ "message": { "content": "hello" } }] });
    assert_eq!(
        canonical_assistant_from_json_response(&response).as_deref(),
        Some("hello")
    );
}

#[test]
fn stream_accumulator_aggregates_tool_call_fragments() {
    let mut accumulator = AssistantAccumulator::default();
    accumulator.consume_data(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"cmd\":"}}]}}]}"#,
    );
    accumulator.consume_data(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
    );
    let streamed = accumulator.finish().expect("tool calls");

    let message = json!({
        "content": null,
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" }
        }]
    });
    let full = canonical_assistant_message(&message).expect("full");
    assert_eq!(streamed, full);
}

#[test]
fn binds_pending_then_reuses_same_id() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);

    let resolution = store.resolve("up", "req-1", &first_turn);
    let (id, capture) = match resolution {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    assert_eq!(id.len(), 32);
    capture.commit(&assistant_hash("hello"));

    let entry = read_file_entries(store.file_path())
        .into_iter()
        .find(|entry| entry.upstream_id == "up" && entry.id == id)
        .expect("persisted entry");
    assert_eq!(entry.user_hash, first_turn.user_anchor_hash);

    let second_turn = fingerprint("hi", Some("hello"));
    match store.resolve("up", "req-2", &second_turn) {
        FallbackResolution::Existing(found) => assert_eq!(found, id),
        _ => panic!("expected existing"),
    }
}

#[test]
fn pending_retry_reuses_id() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);

    let first = match store.resolve("up", "req-1", &first_turn) {
        FallbackResolution::Pending { id, .. } => id,
        _ => panic!("expected pending"),
    };
    let retry = match store.resolve("up", "req-1", &first_turn) {
        FallbackResolution::Pending { id, .. } => id,
        _ => panic!("expected pending"),
    };
    assert_eq!(first, retry);
    assert_eq!(store.inner.pending.read().unwrap().len(), 1);
}

#[test]
fn anchor_fallback_reuses_id_when_assistant_drifts() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first = fingerprint("hi", Some("hello"));
    let id = match store.resolve("up", "req-1", &first) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };

    let drifted = fingerprint("hi", Some("hello, world!"));
    let reused = match store.resolve("up", "req-2", &drifted) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(reused, id);
    let entries = read_file_entries(store.file_path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, id);
}

#[test]
fn pending_by_anchor_completes_binding_without_new_id() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);
    let id = match store.resolve("up", "req-1", &first_turn) {
        FallbackResolution::Pending { id, .. } => id,
        _ => panic!("expected pending"),
    };

    let second_turn = fingerprint("hi", Some("hello"));
    let reused = match store.resolve("up", "req-2", &second_turn) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(reused, id);
    assert_eq!(store.pending_len(), 0);
    let entries = read_file_entries(store.file_path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, id);
}

#[test]
fn present_empty_reuses_id_and_does_not_duplicate() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let fp = empty_assistant_fingerprint("think");

    let first = match store.resolve("up", "req-1", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    let second = match store.resolve("up", "req-2", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(first, second);
    let entries = read_file_entries(store.file_path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, first);
}

#[test]
fn same_id_never_produces_duplicate_file_records() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let fp = fingerprint("hi", Some("hello"));

    let first = match store.resolve("up", "req-1", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    let second = match store.resolve("up", "req-2", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(first, second);
    assert_eq!(read_file_entries(store.file_path()).len(), 1);
}

#[test]
fn legacy_binding_is_migrated_to_new_anchor() {
    let dir = temp_dir();
    let user_text = "<system-reminder>dynamic</system-reminder><user_input>hi there</user_input>";
    let old_user_hash = legacy_user_hash(user_text);
    let old_assistant_hash = legacy_assistant_hash("hello");
    let old_binding = binding_hash(&old_user_hash, &old_assistant_hash);
    let legacy_entry = Entry {
        upstream_id: "up".to_string(),
        hash: old_binding,
        user_hash: String::new(),
        id: "legacy-id".to_string(),
        created_at: 1,
        last_used_at: now_ms(),
    };
    write_file_entries(&dir.join(FALLBACK_FILE_NAME), &[legacy_entry]);
    let store = SessionFallbackStore::new_for_test(&dir);

    let fp = fingerprint(user_text, Some("hello"));
    let found = match store.resolve("up", "req-1", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(found, "legacy-id");
    let entries = read_file_entries(store.file_path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "legacy-id");
    assert_eq!(entries[0].user_hash, fp.user_anchor_hash);
    assert_eq!(entries[0].hash, fp.binding_hash.expect("new binding"));
}

#[test]
fn cross_upstream_isolated() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let fp = fingerprint("hi", Some("hello"));

    let a = match store.resolve("up-a", "req-a", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    let b = match store.resolve("up-b", "req-b", &fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_ne!(a, b);
    assert_eq!(read_file_entries(store.file_path()).len(), 2);
}

#[test]
fn memory_limit_evicts_oldest_and_file_remains() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let mut ids = Vec::new();
    for index in 0..51 {
        let fp = fingerprint(&format!("u{index}"), Some(&format!("a{index}")));
        let id = match store.resolve("up", &format!("req-{index}"), &fp) {
            FallbackResolution::Existing(id) => id,
            _ => panic!("expected existing"),
        };
        ids.push(id);
    }
    assert_eq!(store.inner.memory.read().unwrap().len(), MEMORY_LIMIT);
    assert_eq!(read_file_entries(store.file_path()).len(), 51);

    let oldest_key = Key {
        upstream_id: "up".to_string(),
        binding_hash: binding_hash(&user_hash("u0"), &assistant_hash("a0")),
    };
    assert!(!store
        .inner
        .memory
        .read()
        .unwrap()
        .contains_key(&oldest_key));

    let reload_fp = fingerprint("u0", Some("a0"));
    let loaded = match store.resolve("up", "req-reload", &reload_fp) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    assert_eq!(loaded, ids[0]);
    assert!(store
        .inner
        .memory
        .read()
        .unwrap()
        .contains_key(&oldest_key));
}

#[test]
fn pending_limit_is_bounded() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    for index in 0..51 {
        let fp = fingerprint(&format!("u{index}"), None);
        let _ = store.resolve("up", &format!("req-{index}"), &fp);
    }
    let pending = store.inner.pending.read().unwrap();
    assert_eq!(pending.len(), PENDING_LIMIT);
    assert!(!pending.contains_key(&("req-0".to_string(), "up".to_string())));
}

#[test]
fn debounce_waits_thirty_seconds() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);

    let (id, capture) = match store.resolve("up", "req-1", &first_turn) {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    capture.commit(&assistant_hash("hello"));
    let second_turn = fingerprint("hi", Some("hello"));
    let _ = store.resolve("up", "req-2", &second_turn);
    assert!(store
        .inner
        .memory
        .read()
        .unwrap()
        .values()
        .any(|entry| entry.dirty && entry.entry.id == id));

    store.flush_due_at(Instant::now());
    assert!(store
        .inner
        .memory
        .read()
        .unwrap()
        .values()
        .any(|entry| entry.dirty && entry.entry.id == id));

    store.flush_due_at(Instant::now() + DEBOUNCE + Duration::from_secs(1));
    assert!(!store
        .inner
        .memory
        .read()
        .unwrap()
        .values()
        .any(|entry| entry.dirty && entry.entry.id == id));
}

#[test]
fn cleanup_removes_expired_memory_file_and_pending() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);
    let (_, capture) = match store.resolve("up", "req-1", &first_turn) {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    capture.commit(&assistant_hash("hello"));
    let pending_fp = fingerprint("pending", None);
    let _ = store.resolve("up", "req-pending", &pending_fp);

    {
        let mut memory = store.inner.memory.write().unwrap();
        for entry in memory.values_mut() {
            entry.entry.last_used_at = now_ms().saturating_sub(TTL_MS + 1);
        }
    }
    let mut entries = read_file_entries(store.file_path());
    for entry in &mut entries {
        entry.last_used_at = now_ms().saturating_sub(TTL_MS + 1);
    }
    write_file_entries(store.file_path(), &entries);

    store.cleanup_expired_at(Instant::now() + TTL + Duration::from_secs(1));

    assert!(store.inner.memory.read().unwrap().is_empty());
    assert!(store.inner.pending.read().unwrap().is_empty());
    assert!(read_file_entries(store.file_path()).is_empty());
}

#[test]
fn startup_prunes_expired_and_corrupt_file() {
    let dir = temp_dir();
    let file = dir.join(FALLBACK_FILE_NAME);
    let stale = Entry {
        upstream_id: "up".to_string(),
        hash: "old-hash".to_string(),
        user_hash: String::new(),
        id: "old-id".to_string(),
        created_at: 1,
        last_used_at: 1,
    };
    write_file_entries(&file, &[stale]);
    let store = SessionFallbackStore::new_for_test(&dir);
    assert!(store.inner.memory.read().unwrap().is_empty());
    assert!(read_file_entries(&file).is_empty());

    std::fs::write(&file, b"not json").expect("corrupt file");
    let store = SessionFallbackStore::new_for_test(&dir);
    assert!(store.inner.memory.read().unwrap().is_empty());
    let fp = fingerprint("hi", Some("hello"));
    let _ = store.resolve("up", "req", &fp);
    assert_eq!(read_file_entries(&file).len(), 1);
}

#[tokio::test]
async fn wrapped_stream_preserves_bytes_and_commits_binding() {
    use axum::body::Bytes;
    use futures_util::StreamExt;

    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let first_turn = fingerprint("hi", None);
    let capture = match store.resolve("up", "req-stream", &first_turn) {
        FallbackResolution::Pending { capture, .. } => capture,
        _ => panic!("expected pending"),
    };
    let events = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let bytes = events.as_bytes();
    let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
        Ok(Bytes::copy_from_slice(&bytes[..11])),
        Ok(Bytes::copy_from_slice(&bytes[11..37])),
        Ok(Bytes::copy_from_slice(&bytes[37..])),
    ];
    let input = chunks
        .iter()
        .map(|chunk| chunk.as_ref().expect("chunk").clone())
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut stream = super::stream::wrap_stream(futures_util::stream::iter(chunks), capture);
    while let Some(item) = stream.next().await {
        output.extend_from_slice(item.expect("stream item").as_ref());
    }
    assert_eq!(output, input.concat());

    let expected = binding_hash(&first_turn.user_anchor_hash, &assistant_hash("hello"));
    let raw = std::fs::read_to_string(store.file_path()).expect("mapping file");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("mapping json");
    assert!(value["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .any(|entry| entry["hash"] == expected.as_str()));
}