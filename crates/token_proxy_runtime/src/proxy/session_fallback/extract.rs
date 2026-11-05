//! 首轮 user / assistant 内容提取与两段式指纹。
//!
//! 只服务 `session_fallback`，不参与普通 header_injection 热路径。

use serde_json::Value;
use sha2::{Digest, Sha256};

const USER_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:user:";
const ASSISTANT_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:assistant:";
const BINDING_DOMAIN: &[u8] = b"token-proxy:session-fallback:v1:binding:";

/// 首条 user 与首条 assistant 的规范化内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FirstExchange {
    pub(crate) user: String,
    pub(crate) assistant: Option<String>,
}

/// 从请求体提取首条 user 与首条 assistant；非 messages 结构返回 None。
pub(crate) fn extract_first_exchange(body: &[u8]) -> Option<FirstExchange> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let messages = value.get("messages")?.as_array()?;
    let user_index = messages
        .iter()
        .position(|message| message.get("role").and_then(Value::as_str) == Some("user"))?;
    let user = canonical_content(messages.get(user_index)?.get("content"))?;
    let assistant = messages
        .iter()
        .skip(user_index + 1)
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .and_then(canonical_assistant_message);
    Some(FirstExchange {
        user,
        assistant,
    })
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
    sha256_hex(&[USER_DOMAIN, canonical.as_bytes()])
}

pub(crate) fn assistant_hash(canonical: &str) -> String {
    sha256_hex(&[ASSISTANT_DOMAIN, canonical.as_bytes()])
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
