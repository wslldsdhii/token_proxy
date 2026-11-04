use axum::body::Bytes;
use axum::http::{HeaderName, HeaderValue};

use super::*;

fn marker(id: &str) -> String {
    format!("{SESSION_MARKER_PREFIX}{id}{SESSION_MARKER_SUFFIX}")
}

fn config(header: &'static str) -> HeaderInjectionRuntime {
    HeaderInjectionRuntime {
        target_header: HeaderName::from_static(header),
    }
}

fn body(text: &str) -> ReplayableBody {
    ReplayableBody::from_bytes(Bytes::from(text.to_string()))
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
    apply_config(
        "trae",
        None,
        body(&marker("6aea878b02640bed7e64945d")).as_bytes(),
        &mut headers,
    );
    assert!(headers.is_empty());
}

#[test]
fn apply_injects_target_header() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-session-id");
    apply_config(
        "trae",
        Some(&cfg),
        body(&marker("6aea878b02640bed7e64945d")).as_bytes(),
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
    apply_config(
        "trae",
        Some(&cfg),
        body(&marker("6aea878b02640bed7e64945d")).as_bytes(),
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
    apply_config(
        "trae",
        Some(&cfg),
        body(&marker("6aea878b02640bed7e64945d")).as_bytes(),
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
    apply_config(
        "trae",
        Some(&cfg),
        body("plain chat body").as_bytes(),
        &mut headers,
    );
    assert!(headers.is_empty());
}

#[test]
fn apply_is_idempotent() {
    let mut headers = HeaderMap::new();
    let cfg = config("x-session-id");
    let payload = body(&marker("6aea878b02640bed7e64945d"));
    apply_config("trae", Some(&cfg), payload.as_bytes(), &mut headers);
    apply_config("trae", Some(&cfg), payload.as_bytes(), &mut headers);
    assert_eq!(headers.get_all("x-session-id").iter().count(), 1);
}
