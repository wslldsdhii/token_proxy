use super::extract::{
    assistant_hash, binding_hash, canonical_assistant_from_json_response, canonical_assistant_message,
    extract_first_exchange, user_hash,
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

#[test]
fn extracts_first_user_and_assistant() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "system", "content": "sys" },
        { "role": "user", "content": "hi" },
        { "role": "assistant", "content": "hello" },
        { "role": "user", "content": "again" }
    ])))
    .expect("exchange");
    assert_eq!(exchange.user, "hi");
    assert_eq!(exchange.assistant.as_deref(), Some("hello"));
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
    assert_eq!(exchange.user, "hi\nthere  \nagain");
    assert_eq!(exchange.assistant.as_deref(), Some("ok"));
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
    assert_eq!(exchange.user, "run");
    let assistant = exchange.assistant.expect("tool calls");
    assert!(assistant.contains("call_1"));
    assert!(assistant.contains("bash"));
    assert!(assistant.contains("{\\\"cmd\\\":\\\"ls\\\"}"));
}

#[test]
fn reasoning_only_assistant_is_not_bindable() {
    let exchange = extract_first_exchange(&body(json!([
        { "role": "user", "content": "think" },
        { "role": "assistant", "content": null, "reasoning_content": "hidden" }
    ])))
    .expect("exchange");
    assert_eq!(exchange.user, "think");
    assert!(exchange.assistant.is_none());
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
    let user = user_hash("hi");
    let assistant = assistant_hash("hello");

    let resolution = store.resolve("up", "req-1", &user, None);
    let (id, capture) = match resolution {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    assert_eq!(id.len(), 32);
    capture.commit(&assistant);

    let entry = read_file_entries(store.file_path())
        .into_iter()
        .find(|entry| entry.upstream_id == "up" && entry.hash == binding_hash(&user, &assistant))
        .expect("persisted entry");
    assert_eq!(entry.id, id);

    match store.resolve("up", "req-2", &user, Some(&assistant)) {
        FallbackResolution::Existing(found) => assert_eq!(found, id),
        _ => panic!("expected existing"),
    }
}

#[test]
fn pending_retry_reuses_id() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let user = user_hash("hi");

    let first = match store.resolve("up", "req-1", &user, None) {
        FallbackResolution::Pending { id, .. } => id,
        _ => panic!("expected pending"),
    };
    let retry = match store.resolve("up", "req-1", &user, None) {
        FallbackResolution::Pending { id, .. } => id,
        _ => panic!("expected pending"),
    };
    assert_eq!(first, retry);
    assert_eq!(store.inner.pending.read().unwrap().len(), 1);
}

#[test]
fn cross_upstream_isolated() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let user = user_hash("hi");
    let assistant = assistant_hash("hello");

    let a = match store.resolve("up-a", "req-a", &user, Some(&assistant)) {
        FallbackResolution::Existing(id) => id,
        _ => panic!("expected existing"),
    };
    let b = match store.resolve("up-b", "req-b", &user, Some(&assistant)) {
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
        let user = user_hash(&format!("u{index}"));
        let assistant = assistant_hash(&format!("a{index}"));
        let id = match store.resolve(
            "up",
            &format!("req-{index}"),
            &user,
            Some(&assistant),
        ) {
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

    // 内存满 + miss 时查文件，命中后加载回内存。
    let loaded = match store.resolve(
        "up",
        "req-reload",
        &user_hash("u0"),
        Some(&assistant_hash("a0")),
    ) {
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
        let user = user_hash(&format!("u{index}"));
        let _ = store.resolve("up", &format!("req-{index}"), &user, None);
    }
    let pending = store.inner.pending.read().unwrap();
    assert_eq!(pending.len(), PENDING_LIMIT);
    assert!(!pending.contains_key(&("req-0".to_string(), "up".to_string())));
}

#[test]
fn debounce_waits_thirty_seconds() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let user = user_hash("hi");
    let assistant = assistant_hash("hello");

    let (id, capture) = match store.resolve("up", "req-1", &user, None) {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    capture.commit(&assistant);
    let _ = store.resolve("up", "req-2", &user, Some(&assistant));
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
    let user = user_hash("hi");
    let assistant = assistant_hash("hello");
    let (_, capture) = match store.resolve("up", "req-1", &user, None) {
        FallbackResolution::Pending { id, capture } => (id, capture),
        _ => panic!("expected pending"),
    };
    capture.commit(&assistant);
    let _ = store.resolve("up", "req-pending", &user_hash("pending"), None);

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
    let user = user_hash("hi");
    let _ = store.resolve("up", "req", &user, Some(&assistant_hash("hello")));
    assert_eq!(read_file_entries(&file).len(), 1);
}

#[tokio::test]
async fn wrapped_stream_preserves_bytes_and_commits_binding() {
    use axum::body::Bytes;
    use futures_util::StreamExt;

    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let user = user_hash("hi");
    let capture = match store.resolve("up", "req-stream", &user, None) {
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

    let expected = binding_hash(&user, &assistant_hash("hello"));
    let raw = std::fs::read_to_string(store.file_path()).expect("mapping file");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("mapping json");
    assert!(value["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .any(|entry| entry["hash"] == expected.as_str()));
}
