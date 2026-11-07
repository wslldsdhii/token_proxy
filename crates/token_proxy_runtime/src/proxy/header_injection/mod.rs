//! header_injection —— 提取客户端已有的会话标识，或按需注入目标请求头。
//!
//! 优先级（高 → 低）：
//! 1. 目标 Header 已存在 → 零注入、零分配；
//! 2. 客户端已有任一会话头 → 零注入、零分配；
//! 3. 请求体钩子标记；
//! 4. 请求体显式会话字段（`prompt_cache_key` 等）；
//! 5. 回退指纹（仅在 `fallback_enabled=true` 时）。
//!
//! 标记检测只做文本扫描，不解析 JSON；只有需要 body 会话字段或回退时才解析一次。

use axum::http::{HeaderMap, HeaderValue};

use super::config::{header_injection::HeaderInjectionRuntime, UpstreamRuntime};
use super::request_body::ReplayableBody;
use super::session_fallback::{
    extract as fallback_extract, FallbackResolution, SessionFallbackCapture, SessionFallbackStore,
};

/// 钩子输出标记的固定前缀（与 `myplugins/trae-hooks` 契约一致）。
pub(super) const SESSION_MARKER_PREFIX: &str = "[[token-proxy-session-v1:";
/// 标记终止符。
pub(super) const SESSION_MARKER_SUFFIX: &str = "]]";
const MIN_SESSION_ID_LEN: usize = 8;
const MAX_SESSION_ID_LEN: usize = 64;
/// 前缀之后允许扫描的窗口：足够容纳「空白 + 64 位 ID + 终止符」。
const MARKER_WINDOW_BYTES: usize = 128;
/// 客户端显式会话头；任一存在即视为客户端已有会话身份，注入层保持零副作用。
const CLIENT_SESSION_HEADERS: &[&str] = &[
    "x-opencode-session",
    "x-session-id",
    "session-id",
    "session_id",
    "x-conversation-id",
    "thread-id",
];

/// 出站 Header 注入入口：`prepare_upstream_request_with_body` 在 `build_request_headers` 之后调用。
///
/// 返回 `Some(capture)` 时表示首轮回退 ID 已注入，响应侧需要用该 capture 提交绑定。
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    fallback_store: Option<&SessionFallbackStore>,
    upstream: &UpstreamRuntime,
    body: &ReplayableBody,
    headers: &mut HeaderMap,
    request_id: &str,
) -> Option<SessionFallbackCapture> {
    apply_config(
        upstream.id.as_str(),
        upstream.header_injection.as_ref(),
        fallback_store,
        body.as_bytes(),
        headers,
        request_id,
    )
}

fn apply_config(
    upstream_id: &str,
    config: Option<&HeaderInjectionRuntime>,
    fallback_store: Option<&SessionFallbackStore>,
    body: &[u8],
    headers: &mut HeaderMap,
    request_id: &str,
) -> Option<SessionFallbackCapture> {
    let Some(config) = config else {
        return None;
    };
    if body.is_empty() {
        return None;
    }

    // 目标 Header 已存在（客户端显式或静态复写命中）：不检测、不分配、不落盘。
    if headers.contains_key(&config.target_header) {
        tracing::debug!(
            upstream_id,
            header = %config.target_header,
            "header injection skipped: target header already present"
        );
        return None;
    }

    // 客户端已有任一会话头：优先于标记与回退，避免同一请求出现两个会话身份。
    if let Some(client_header) = CLIENT_SESSION_HEADERS
        .iter()
        .find(|name| headers.contains_key(**name))
    {
        tracing::debug!(
            upstream_id,
            client_header,
            "header injection skipped: client session header already present"
        );
        return None;
    }

    if let Some(session_id) = extract_session_id(body) {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            headers.insert(config.target_header.clone(), value);
            tracing::debug!(
                upstream_id,
                header = %config.target_header,
                session_id,
                "injected session header from hook marker"
            );
            return None;
        }
        tracing::warn!(
            upstream_id,
            "header injection skipped: session id is not a valid header value"
        );
    }

    let may_have_body_session = fallback_extract::body_may_contain_session_key(body);
    if !config.fallback_enabled && !may_have_body_session {
        tracing::debug!(
            upstream_id,
            "header injection enabled but no session marker found"
        );
        return None;
    }

    let Some(hints) = fallback_extract::extract_request_hints(body) else {
        tracing::debug!(
            upstream_id,
            "header injection skipped: request body is not JSON"
        );
        return None;
    };

    if let Some(session_id) = hints.body_session_id.as_deref() {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            headers.insert(config.target_header.clone(), value);
            tracing::debug!(
                upstream_id,
                header = %config.target_header,
                session_id,
                "injected session header from request body"
            );
            return None;
        }
        tracing::warn!(
            upstream_id,
            "header injection skipped: body session id is not a valid header value"
        );
    }

    if !config.fallback_enabled {
        tracing::debug!(upstream_id, "session fallback disabled");
        return None;
    }

    let Some(store) = fallback_store else {
        tracing::debug!(upstream_id, "session fallback skipped: store unavailable");
        return None;
    };
    let Some(exchange) = hints.exchange else {
        tracing::debug!(
            upstream_id,
            "session fallback skipped: no first user/assistant exchange"
        );
        return None;
    };

    let fingerprint = exchange.fingerprint();
    let (id, capture) = match store.resolve(upstream_id, request_id, &fingerprint) {
        FallbackResolution::Existing(id) => (id, None),
        FallbackResolution::Pending { id, capture } => (id, Some(capture)),
    };

    let Ok(value) = HeaderValue::from_str(&id) else {
        tracing::warn!(
            upstream_id,
            "session fallback skipped: generated id is not a valid header value"
        );
        return None;
    };
    headers.insert(config.target_header.clone(), value);
    tracing::debug!(
        upstream_id,
        header = %config.target_header,
        id,
        pending = capture.is_some(),
        "injected session fallback header"
    );
    capture
}

/// 返回请求体文本中**最后一次**合法标记里的会话 ID。
pub(super) fn extract_session_id(body: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(body).ok()?;
    let mut search_from = 0usize;
    let mut last: Option<&str> = None;

    while let Some(relative) = text[search_from..].find(SESSION_MARKER_PREFIX) {
        let window_start = search_from + relative + SESSION_MARKER_PREFIX.len();
        if window_start >= text.len() {
            break;
        }
        let window_end =
            floor_char_boundary(text, (window_start + MARKER_WINDOW_BYTES).min(text.len()));
        if let Some(id) = parse_session_id(&text[window_start..window_end]) {
            last = Some(id);
        }
        search_from = window_start;
    }

    last
}

fn parse_session_id(window: &str) -> Option<&str> {
    let rest = window.trim_start();
    let id_len = rest
        .find(|c: char| !is_session_id_char(c))
        .unwrap_or(rest.len());
    let (id, tail) = rest.split_at(id_len);
    if !tail.starts_with(SESSION_MARKER_SUFFIX) {
        return None;
    }
    if id.len() < MIN_SESSION_ID_LEN || id.len() > MAX_SESSION_ID_LEN {
        return None;
    }
    Some(id)
}

fn is_session_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `str` 切片必须落在 char 边界；窗口末尾可能截断多字节字符。
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
