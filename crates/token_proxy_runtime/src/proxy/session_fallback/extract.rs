//! 首轮 user / assistant 内容提取与两段式指纹。
//!
//! 只服务 `session_fallback`，不参与普通 header_injection 热路径。

use serde_json::Value;
use sha2::{Digest, Sha256};

const USER_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:user:";
const ASSISTANT_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:assistant:";
const BINDING_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:binding:";
const ASSISTANT_EMPTY_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:assistant-empty";
const USER_INPUT_OPEN: &str = "<user_input>";
const USER_INPUT_CLOSE: &str = "</user_input>";
const BODY_SESSION_KEYS: &[&str] = &[
    "prompt_cache_key",
    "conversation_id",
    "session_id",
    "thread_id",
    "chat_id",
];

/// 首条 assistant 的三种状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FirstAssistant {
    /// 首条 user 之后没有 assistant 消息：真正的首轮。
    Absent,
    /// 有 assistant 消息，且正文或 tool_calls 可规范化。
    Present(String),
    /// 有 assistant 消息，但正文与 tool_calls 都为空（reasoning-only / 空回复）。
    PresentEmpty,
}

/// 首条 user 与首条 assistant 的规范化内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FirstExchange {
    /// 整段首条 user 规范化文本（旧算法兼容用）。
    pub(crate) user_full: String,
    /// 稳定锚点：最后一个 `<user_input>` 内容，或整段文本。
    pub(crate) user_anchor: String,
    pub(crate) assistant: FirstAssistant,
}

/// 一次请求中可用于会话识别的线索；只解析一次 JSON。
#[derive(Clone, Debug, Default)]
pub(crate) struct RequestHints {
    /// 请求体显式会话字段（`prompt_cache_key` 等）。
    pub(crate) body_session_id: Option<String>,
    pub(crate) exchange: Option<FirstExchange>,
}

/// 供 `SessionFallbackStore` 使用的指纹集合（新旧两套算法 + 锚点）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionFingerprint {
    pub(crate) user_anchor_hash: String,
    pub(crate) binding_hash: Option<String>,
    pub(crate) legacy_binding_hash: Option<String>,
    pub(crate) assistant_hash: Option<String>,
    pub(crate) has_assistant_message: bool,
    pub(crate) degraded: bool,
}

/// 从请求体提取首条 user / assistant 与 body 会话字段；非 JSON 返回 None。
pub(crate) fn extract_request_hints(body: &[u8]) -> Option<RequestHints> {
    let value: Value = serde_json::from_slice(body).ok()?;
    Some(extract_request_hints_from_value(&value))
}

/// 低成本预筛：body 是否可能包含显式会话字段名，避免不必要的大 JSON 解析。
pub(crate) fn body_may_contain_session_key(body: &[u8]) -> bool {
    BODY_SESSION_KEYS
        .iter()
        .any(|key| contains_ascii_case_insensitive(body, key.as_bytes()))
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

pub(crate) fn extract_request_hints_from_value(value: &Value) -> RequestHints {
    RequestHints {
        body_session_id: body_session_id(value),
        exchange: extract_first_exchange_from_value(value),
    }
}

/// 从请求体提取首条 user 与首条 assistant；非 messages 结构返回 None。
pub(crate) fn extract_first_exchange(body: &[u8]) -> Option<FirstExchange> {
    let value: Value = serde_json::from_slice(body).ok()?;
    extract_first_exchange_from_value(&value)
}

pub(crate) fn extract_first_exchange_from_value(value: &Value) -> Option<FirstExchange> {
    let messages = value.get("messages")?.as_array()?;
    let user_index = messages
        .iter()
        .position(|message| message.get("role").and_then(Value::as_str) == Some("user"))?;
    let user_full = canonical_content(messages.get(user_index)?.get("content"))?;
    let user_anchor = stable_user_anchor(&user_full);
    let assistant = match messages
        .iter()
        .skip(user_index + 1)
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
    {
        Some(message) => match canonical_assistant_message(message) {
            Some(text) => FirstAssistant::Present(text),
            None => FirstAssistant::PresentEmpty,
        },
        None => FirstAssistant::Absent,
    };
    Some(FirstExchange {
        user_full,
        user_anchor,
        assistant,
    })
}

impl FirstExchange {
    pub(crate) fn fingerprint(&self) -> SessionFingerprint {
        let user_anchor_hash = user_hash(&self.user_anchor);
        let legacy_user_hash = legacy_user_hash(&self.user_full);
        match &self.assistant {
            FirstAssistant::Absent => SessionFingerprint {
                user_anchor_hash,
                binding_hash: None,
                legacy_binding_hash: None,
                assistant_hash: None,
                has_assistant_message: false,
                degraded: false,
            },
            FirstAssistant::Present(text) => {
                let assistant_hash = assistant_hash(text);
                let legacy_assistant_hash = legacy_assistant_hash(text);
                SessionFingerprint {
                    binding_hash: Some(binding_hash(&user_anchor_hash, &assistant_hash)),
                    legacy_binding_hash: Some(binding_hash(
                        &legacy_user_hash,
                        &legacy_assistant_hash,
                    )),
                    user_anchor_hash,
                    assistant_hash: Some(assistant_hash),
                    has_assistant_message: true,
                    degraded: false,
                }
            }
            FirstAssistant::PresentEmpty => {
                let assistant_hash = assistant_empty_hash();
                SessionFingerprint {
                    binding_hash: Some(binding_hash(&user_anchor_hash, &assistant_hash)),
                    legacy_binding_hash: None,
                    user_anchor_hash,
                    assistant_hash: Some(assistant_hash),
                    has_assistant_message: true,
                    degraded: true,
                }
            }
        }
    }
}

/// 规范化 assistant 消息：优先正文，无正文时按 tool_calls 稳定序列化。
pub(crate) fn canonical_assistant_message(message: &Value) -> Option<String> {
    if let Some(text) = canonical_content(message.get("content")) {
        return Some(text);
    }
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .and_then(|calls| canonical_tool_calls(calls))
}

/// 从完整 JSON 响应中取首条 assistant 内容（OpenAI chat completions）。
pub(crate) fn canonical_assistant_from_json_response(value: &Value) -> Option<String> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(canonical_assistant_message)
}

pub(crate) fn user_hash(canonical: &str) -> String {
    let stripped = strip_invisible(canonical);
    sha256_hex(&[USER_DOMAIN, stripped.as_bytes()])
}

pub(crate) fn assistant_hash(canonical: &str) -> String {
    let stripped = strip_invisible(canonical);
    sha256_hex(&[ASSISTANT_DOMAIN, stripped.as_bytes()])
}

/// 空 assistant 的固定摘要；用于 `PresentEmpty` 的降级绑定。
pub(crate) fn assistant_empty_hash() -> String {
    sha256_hex(&[ASSISTANT_EMPTY_DOMAIN])
}

pub(crate) fn legacy_user_hash(canonical: &str) -> String {
    let normalized = legacy_normalize_text(canonical);
    sha256_hex(&[USER_DOMAIN, normalized.as_bytes()])
}

pub(crate) fn legacy_assistant_hash(canonical: &str) -> String {
    let normalized = legacy_normalize_text(canonical);
    sha256_hex(&[ASSISTANT_DOMAIN, normalized.as_bytes()])
}

/// 两段式绑定哈希；长度前缀避免拼接歧义。
pub(crate) fn binding_hash(user_hash: &str, assistant_hash: &str) -> String {
    let user_len = (user_hash.len() as u32).to_be_bytes();
    let assistant_len = (assistant_hash.len() as u32).to_be_bytes();
    sha256_hex(&[
        BINDING_DOMAIN,
        &user_len,
        user_hash.as_bytes(),
        &assistant_len,
        assistant_hash.as_bytes(),
    ])
}

/// 稳定锚点：最后一个 `<user_input>` 内容；没有标签时退回整段文本。
pub(crate) fn stable_user_anchor(text: &str) -> String {
    let inner = last_user_input(text).unwrap_or(text);
    strip_invisible(inner)
}

/// 删除全部 Unicode 空白与常见零宽字符，只保留可见字符。
pub(crate) fn strip_invisible(text: &str) -> String {
    text.chars()
        .filter(|character| {
            !character.is_whitespace()
                && !matches!(
                    character,
                    '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
                )
        })
        .collect()
}

fn last_user_input(text: &str) -> Option<&str> {
    let mut search_from = 0usize;
    let mut last = None;
    while let Some(open) = find_ascii_case_insensitive(text, USER_INPUT_OPEN, search_from) {
        let content_start = open + USER_INPUT_OPEN.len();
        let Some(close) = find_ascii_case_insensitive(text, USER_INPUT_CLOSE, content_start)
        else {
            break;
        };
        let candidate = &text[content_start..close];
        if !candidate.trim().is_empty() {
            last = Some(candidate);
        }
        search_from = close + USER_INPUT_CLOSE.len();
    }
    last
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || from >= haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len())
        .find(|&index| haystack[index..index + needle.len()].eq_ignore_ascii_case(needle))
}

fn body_session_id(value: &Value) -> Option<String> {
    if let Some(object) = value.as_object() {
        if let Some(id) = session_id_from_object(object) {
            return Some(id);
        }
        if let Some(metadata) = object.get("metadata").and_then(Value::as_object) {
            if let Some(id) = session_id_from_object(metadata) {
                return Some(id);
            }
        }
    }
    None
}

fn session_id_from_object(object: &serde_json::Map<String, Value>) -> Option<String> {
    for key in BODY_SESSION_KEYS {
        let Some(value) = object.get(*key).and_then(Value::as_str) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value.len() > 256 || value.chars().any(|c| c.is_control()) {
            continue;
        }
        return Some(value.to_string());
    }
    None
}

fn canonical_content(content: Option<&Value>) -> Option<String> {
    let content = content?;
    let mut parts = Vec::new();
    match content {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(text) => parts.push(text.clone()),
                    Value::Object(object) => {
                        if let Some(text) = object.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        } else if let Some(text) =
                            object.get("input_text").and_then(Value::as_str)
                        {
                            parts.push(text.to_string());
                        } else if let Some(text) =
                            object.get("output_text").and_then(Value::as_str)
                        {
                            parts.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        _ => {}
    }
    let normalized = normalize_text(&parts.join("\n"));
    (!normalized.is_empty()).then_some(normalized)
}

pub(crate) fn canonical_tool_calls(calls: &[Value]) -> Option<String> {
    if calls.is_empty() {
        return None;
    }
    let mut normalized: Vec<(i64, Value)> = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let order = call
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(index as i64);
            let id = call.get("id").and_then(Value::as_str).unwrap_or("");
            let kind = call
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("function");
            let function = call.get("function");
            let name = function
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let arguments = function
                .and_then(|value| value.get("arguments"))
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .unwrap_or_default();
            (
                order,
                serde_json::json!({
                    "id": id,
                    "type": kind,
                    "function": { "name": name, "arguments": arguments },
                }),
            )
        })
        .collect();
    normalized.sort_by_key(|(order, _)| *order);
    let value = Value::Array(normalized.into_iter().map(|(_, value)| value).collect());
    serde_json::to_string(&value).ok()
}

pub(crate) fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").trim().to_string()
}

/// 旧算法使用的规范化（仅 CRLF→LF + 首尾 trim），不得改变。
fn legacy_normalize_text(text: &str) -> String {
    normalize_text(text)
}

fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(messages: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "messages": messages })).expect("body")
    }

    #[test]
    fn extracts_last_user_input_anchor_and_ignores_dynamic_reminders() {
        let first = extract_first_exchange(&body(json!([
            { "role": "user", "content": [
                { "type": "text", "text": "<system-reminder>terminal A</system-reminder>" },
                { "type": "text", "text": "<system-reminder>todo A</system-reminder>" },
                { "type": "text", "text": "<user_input>hi</user_input>" }
            ]},
            { "role": "assistant", "content": "hello" }
        ])))
        .expect("first");
        let second = extract_first_exchange(&body(json!([
            { "role": "user", "content": [
                { "type": "text", "text": "<system-reminder>terminal B</system-reminder>" },
                { "type": "text", "text": "<system-reminder>todo B</system-reminder>" },
                { "type": "text", "text": "<user_input>hi</user_input>" }
            ]},
            { "role": "assistant", "content": "hello" }
        ])))
        .expect("second");

        assert_eq!(first.user_anchor, "hi");
        assert_eq!(second.user_anchor, "hi");
        assert_eq!(first.fingerprint().binding_hash, second.fingerprint().binding_hash);
    }

    #[test]
    fn multiple_user_inputs_take_the_last() {
        let exchange = extract_first_exchange(&body(json!([
            { "role": "user", "content": "<user_input>first</user_input><user_input>second</user_input>" }
        ])))
        .expect("exchange");
        assert_eq!(exchange.user_anchor, "second");
    }

    #[test]
    fn missing_user_input_falls_back_to_full_text() {
        let exchange = extract_first_exchange(&body(json!([
            { "role": "user", "content": "  plain  input  " }
        ])))
        .expect("exchange");
        assert_eq!(exchange.user_anchor, "plaininput");
    }

    #[test]
    fn strips_all_whitespace_and_zero_width_characters() {
        let exchange = extract_first_exchange(&body(json!([
            { "role": "user", "content": "<user_input>h\u{200B}i\n\t t h e r e</user_input>" }
        ])))
        .expect("exchange");
        assert_eq!(exchange.user_anchor, "hithere");
    }

    #[test]
    fn assistant_present_empty_is_distinguished_from_absent() {
        let present_empty = extract_first_exchange(&body(json!([
            { "role": "user", "content": "think" },
            { "role": "assistant", "content": null, "reasoning_content": "hidden" }
        ])))
        .expect("present empty");
        assert_eq!(present_empty.assistant, FirstAssistant::PresentEmpty);

        let absent = extract_first_exchange(&body(json!([
            { "role": "user", "content": "think" }
        ])))
        .expect("absent");
        assert_eq!(absent.assistant, FirstAssistant::Absent);
    }

    #[test]
    fn handles_string_and_array_content() {
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
        assert_eq!(exchange.assistant, FirstAssistant::Present("ok".to_string()));
    }

    #[test]
    fn tool_call_hashes_ignore_whitespace() {
        let first = canonical_assistant_message(&json!({
            "content": null,
            "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "bash", "arguments": "{\"cmd\": \"ls\"}" }
            }]
        }))
        .expect("tool calls");
        let second = canonical_assistant_message(&json!({
            "content": null,
            "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" }
            }]
        }))
        .expect("tool calls");
        assert_eq!(assistant_hash(&first), assistant_hash(&second));
    }

    #[test]
    fn extracts_body_session_id_from_top_level_and_metadata() {
        let hints = extract_request_hints_from_value(&json!({
            "prompt_cache_key": "sess-abc",
            "messages": [{ "role": "user", "content": "hi" }]
        }));
        assert_eq!(hints.body_session_id.as_deref(), Some("sess-abc"));

        let hints = extract_request_hints_from_value(&json!({
            "metadata": { "conversation_id": "conv-1" },
            "messages": [{ "role": "user", "content": "hi" }]
        }));
        assert_eq!(hints.body_session_id.as_deref(), Some("conv-1"));
    }
}