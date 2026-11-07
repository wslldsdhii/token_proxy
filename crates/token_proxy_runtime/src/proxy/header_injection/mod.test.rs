use axum::body::Bytes;
use axum::http::{HeaderName, HeaderValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn marker(id: &str) -> String {
    format!("{SESSION_MARKER_PREFIX}{id}{SESSION_MARKER_SUFFIX}")
}

fn config(header: &'static str) -> HeaderInjectionRuntime {
    HeaderInjectionRuntime {
        target_header: HeaderName::from_static(header),
        fallback_enabled: false,
    }
}

fn config_with_fallback(header: &'static str) -> HeaderInjectionRuntime {
    HeaderInjectionRuntime {
        target_header: HeaderName::from_static(header),
        fallback_enabled: true,
    }
}

fn body(text: &str) -> ReplayableBody {
    ReplayableBody::from_bytes(Bytes::from(text.to_string()))
}

fn chat_body(first_user: &str, first_assistant: Option<&str>) -> ReplayableBody {
    let mut messages = vec![serde_json::json!({
        "role": "user",
        "content": first_user,
    })];
    if let Some(assistant) = first_assistant {
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": assistant,
        }));
    }
    body(
        &serde_json::to_string(&serde_json::json!({ "messages": messages }))
            .expect("chat body"),
    )
}

fn apply(
    cfg: Option<&HeaderInjectionRuntime>,
    payload: &ReplayableBody,
    headers: &mut HeaderMap,
) -> Option<SessionFallbackCapture> {
    apply_config("trae", cfg, None, payload.as_bytes(), headers, "req-test")
}

fn apply_with_fallback(
    store: &SessionFallbackStore,
    cfg: &HeaderInjectionRuntime,
    payload: &ReplayableBody,
    headers: &mut HeaderMap,
) -> Option<SessionFallbackCapture> {
    apply_with_fallback_request(store, cfg, payload, headers, "req-fallback")
}

fn apply_with_fallback_request(
    store: &SessionFallbackStore,
    cfg: &HeaderInjectionRuntime,
    payload: &ReplayableBody,
    headers: &mut HeaderMap,
    request_id: &str,
) -> Option<SessionFallbackCapture> {
    apply_config(
        "trae",
        Some(cfg),
        Some(store),
        payload.as_bytes(),
        headers,
        request_id,
    )
}

fn temp_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "tp-header-injection-fallback-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

#[test]
fn extracts_single_marker() {
    let text = format!("before {} after", marker("6aea878b02640bed7e64945d"));
    assert_eq!(
        extract_session_id(text.as_bytes()),
        Some("6aea878b02640bed7e64945d")
    );
}

#[test]
fn last_marker_wins() {
    let text = format!(
        "history {} middle {}",
        marker("aaaaaaaaaaaaaaaa"),
        marker("bbbbbbbbbbbbbbbb")
    );
    assert_eq!(
        extract_session_id(text.as_bytes()),
        Some("bbbbbbbbbbbbbbbb")
    );
}

#[test]
fn handles_json_escaped_text() {
    // 钩子 stdout 被包进 <text_stdout>，再作为 JSON 字符串值转义；标记本身不含转义字符。
    let text = format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"<hook><text_stdout>{}...\"}}]}}",
        marker("6aea878b02640bed7e64945d")
    );
    assert_eq!(
        extract_session_id(text.as_bytes()),
        Some("6aea878b02640bed7e64945d")
    );
}

#[test]
fn accepts_whitespace_after_prefix() {
    let text = format!("{SESSION_MARKER_PREFIX}   6aea878b02640bed7e64945d]]");
    assert_eq!(
        extract_session_id(text.as_bytes()),
        Some("6aea878b02640bed7e64945d")
    );
}

#[test]
fn rejects_marker_without_terminator() {
    let text = format!("{SESSION_MARKER_PREFIX}6aea878b02640bed7e64945d");
    assert_eq!(extract_session_id(text.as_bytes()), None);
}

#[test]
fn rejects_short_id() {
    let text = format!("{SESSION_MARKER_PREFIX}short]]");
    assert_eq!(extract_session_id(text.as_bytes()), None);
}

#[test]
fn rejects_invalid_characters() {
    let text = format!("{SESSION_MARKER_PREFIX}abc!defghij]]");
    assert_eq!(extract_session_id(text.as_bytes()), None);
}

#[test]
fn rejects_overlong_id() {
    let id = "a".repeat(MAX_SESSION_ID_LEN + 1);
    let text = format!("{SESSION_MARKER_PREFIX}{id}]]");
    assert_eq!(extract_session_id(text.as_bytes()), None);
}

#[test]
fn ignores_non_utf8_body() {
    assert_eq!(extract_session_id(&[0xff, 0xfe, 0x00]), None);
}

#[test]
fn ignores_empty_body() {
    assert_eq!(extract_session_id(b""), None);
}

#[test]
fn apply_is_noop_when_disabled() {
    let mut headers = HeaderMap::new();
    apply(
        None,
        &body(&marker("6aea878b02640bed7e64945d")),
        &mut headers,
    );
    assert!(headers.is_empty());
}

#[test]
fn apply_injects_target_header() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-session-id");
    apply(
        Some(&cfg),
        &body(&marker("6aea878b02640bed7e64945d")),
        &mut headers,
    );
    assert_eq!(
        headers.get("x-session-id"),
        Some(&HeaderValue::from_static("6aea878b02640bed7e64945d"))
    );
}

#[test]
fn apply_supports_opencode_header() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-opencode-session");
    apply(
        Some(&cfg),
        &body(&marker("6aea878b02640bed7e64945d")),
        &mut headers,
    );
    assert_eq!(
        headers.get("x-opencode-session"),
        Some(&HeaderValue::from_static("6aea878b02640bed7e64945d"))
    );
    assert!(headers.get("x-session-id").is_none());
}

#[test]
fn apply_preserves_existing_header() {
    let mut headers = HeaderMap::new();
    headers.insert("x-session-id", HeaderValue::from_static("client-value"));
    let cfg = config("x-session-id");
    apply(
        Some(&cfg),
        &body(&marker("6aea878b02640bed7e64945d")),
        &mut headers,
    );
    assert_eq!(
        headers.get("x-session-id"),
        Some(&HeaderValue::from_static("client-value"))
    );
}

#[test]
fn apply_without_marker_changes_nothing() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-session-id");
    apply(Some(&cfg), &body("plain chat body"), &mut headers);
    assert!(headers.is_empty());
}

#[test]
fn apply_is_idempotent() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-session-id");
    let payload = body(&marker("6aea878b02640bed7e64945d"));
    apply(Some(&cfg), &payload, &mut headers);
    apply(Some(&cfg), &payload, &mut headers);
    assert_eq!(headers.get_all("x-session-id").iter().count(), 1);
}

#[test]
fn fallback_allocates_on_first_turn_and_reuses_on_second() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(
        &store,
        &cfg,
        &chat_body("hi", None),
        &mut headers,
    )
    .expect("pending capture");
    let first_id = headers
        .get("x-opencode-session")
        .expect("fallback header")
        .to_str()
        .expect("ascii")
        .to_string();
    assert_eq!(first_id.len(), 32);
    assert_eq!(store.pending_len(), 1);

    // 首轮回复到达后提交绑定。
    let assistant = fallback_extract::assistant_hash("hello");
    capture.commit(&assistant);

    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(
        &store,
        &cfg,
        &chat_body("hi", Some("hello")),
        &mut headers,
    );
    assert!(capture.is_none());
    assert_eq!(
        headers.get("x-opencode-session").expect("reused header"),
        first_id.as_str()
    );
}

#[test]
fn fallback_skipped_when_any_session_header_present() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-session-id");
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-opencode-session",
        HeaderValue::from_static("client-session"),
    );
    assert!(apply_with_fallback(
        &store,
        &cfg,
        &chat_body("hi", None),
        &mut headers
    )
    .is_none());
    assert!(headers.get("x-session-id").is_none());
}

#[test]
fn marker_wins_over_fallback() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-session-id");
    let mut headers = HeaderMap::new();
    let payload = body(&format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"hi {}\"}}]}}",
        marker("6aea878b02640bed7e64945d")
    ));
    let capture = apply_with_fallback(&store, &cfg, &payload, &mut headers);
    assert!(capture.is_none());
    assert_eq!(
        headers.get("x-session-id"),
        Some(&HeaderValue::from_static("6aea878b02640bed7e64945d"))
    );
    assert_eq!(store.pending_len(), 0);
}

#[test]
fn fallback_skipped_without_first_user() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-session-id");
    let mut headers = HeaderMap::new();
    let payload = body("{\"messages\":[]}");
    assert!(apply_with_fallback(&store, &cfg, &payload, &mut headers).is_none());
    assert!(headers.is_empty());
}

fn marker_body(marker_id: &str) -> ReplayableBody {
    body(&format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"hi {}\"}}]}}",
        marker(marker_id)
    ))
}

fn reminder_chat_body(reminder: &str, first_assistant: Option<&str>) -> ReplayableBody {
    let mut messages = vec![serde_json::json!({
        "role": "user",
        "content": [
            { "type": "text", "text": format!("<system-reminder>{reminder}</system-reminder>") },
            { "type": "text", "text": "<user_input>hi</user_input>" }
        ],
    })];
    if let Some(assistant) = first_assistant {
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": assistant,
        }));
    }
    body(
        &serde_json::to_string(&serde_json::json!({ "messages": messages }))
            .expect("chat body"),
    )
}

#[test]
fn marker_skipped_when_client_session_header_present() {
    let cfg = config("x-session-id");
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-opencode-session",
        HeaderValue::from_static("client-session"),
    );
    apply(
        Some(&cfg),
        &marker_body("6aea878b02640bed7e64945d"),
        &mut headers,
    );
    assert!(headers.get("x-session-id").is_none());
    assert_eq!(
        headers.get("x-opencode-session"),
        Some(&HeaderValue::from_static("client-session"))
    );
}

#[test]
fn any_client_session_header_blocks_fallback() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-session-id");
    let mut headers = HeaderMap::new();
    headers.insert("thread-id", HeaderValue::from_static("thread-1"));
    assert!(apply_with_fallback(
        &store,
        &cfg,
        &chat_body("hi", None),
        &mut headers
    )
    .is_none());
    assert!(headers.get("x-session-id").is_none());
    assert_eq!(store.pending_len(), 0);
}

#[test]
fn target_header_present_does_not_allocate() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-opencode-session",
        HeaderValue::from_static("client-session"),
    );
    assert!(apply_with_fallback(
        &store,
        &cfg,
        &chat_body("hi", None),
        &mut headers
    )
    .is_none());
    assert_eq!(store.pending_len(), 0);
    assert_eq!(store.file_entry_count(), 0);
}

#[test]
fn body_session_field_is_injected_without_fallback_allocation() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let payload = body(
        &serde_json::to_string(&serde_json::json!({
            "prompt_cache_key": "sess-abc",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .expect("body"),
    );
    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(&store, &cfg, &payload, &mut headers);
    assert!(capture.is_none());
    assert_eq!(
        headers.get("x-opencode-session"),
        Some(&HeaderValue::from_static("sess-abc"))
    );
    assert_eq!(store.pending_len(), 0);
}

#[test]
fn body_session_field_works_when_fallback_disabled() {
    let cfg = config("x-opencode-session");
    let payload = body(
        &serde_json::to_string(&serde_json::json!({
            "conversation_id": "conv-1",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .expect("body"),
    );
    let mut headers = HeaderMap::new();
    apply(Some(&cfg), &payload, &mut headers);
    assert_eq!(
        headers.get("x-opencode-session"),
        Some(&HeaderValue::from_static("conv-1"))
    );
}

#[test]
fn invalid_body_session_field_falls_back() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let payload = body(
        &serde_json::to_string(&serde_json::json!({
            "session_id": "bad\nvalue",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .expect("body"),
    );
    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(&store, &cfg, &payload, &mut headers);
    assert!(capture.is_some());
    assert_eq!(
        headers
            .get("x-opencode-session")
            .expect("fallback header")
            .to_str()
            .expect("ascii")
            .len(),
        32
    );
    assert_eq!(store.pending_len(), 1);
}

#[test]
fn dynamic_reminders_reuse_same_fallback_id() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(
        &store,
        &cfg,
        &reminder_chat_body("terminal A", None),
        &mut headers,
    )
    .expect("pending capture");
    let first_id = headers
        .get("x-opencode-session")
        .expect("fallback header")
        .to_str()
        .expect("ascii")
        .to_string();
    capture.commit(&fallback_extract::assistant_hash("hello"));

    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback(
        &store,
        &cfg,
        &reminder_chat_body("terminal B", Some("hello")),
        &mut headers,
    );
    assert!(capture.is_none());
    assert_eq!(
        headers.get("x-opencode-session").expect("reused header"),
        first_id.as_str()
    );
}

#[test]
fn pending_binds_from_next_request_history() {
    let dir = temp_dir();
    let store = SessionFallbackStore::new_for_test(&dir);
    let cfg = config_with_fallback("x-opencode-session");
    let mut headers = HeaderMap::new();
    let _capture = apply_with_fallback_request(
        &store,
        &cfg,
        &chat_body("hi", None),
        &mut headers,
        "req-first",
    )
    .expect("pending capture");
    let first_id = headers
        .get("x-opencode-session")
        .expect("fallback header")
        .to_str()
        .expect("ascii")
        .to_string();

    // 首轮响应未提交（capture 被丢弃），下一轮历史已带 assistant。
    let mut headers = HeaderMap::new();
    let capture = apply_with_fallback_request(
        &store,
        &cfg,
        &chat_body("hi", Some("hello")),
        &mut headers,
        "req-next",
    );
    assert!(capture.is_none());
    assert_eq!(
        headers.get("x-opencode-session").expect("reused header"),
        first_id.as_str()
    );
    assert_eq!(store.pending_len(), 0);
}
