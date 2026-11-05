//! header_injection —— 从请求体提取 Trae 钩子写入的会话标记，并注入目标请求头。
//!
//! 设计要点见 `openspec/changes/add-my-header-injection/design.md` 决策 6：
//! - 只做文本扫描，不解析 JSON（请求体可能 100MB 级），无新增依赖；
//! - 取**最后一次**出现的合法标记（多轮历史里最新 user 消息在文档末尾）；
//! - 不修改请求体/响应，目标 Header 已存在时不覆盖。

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

    if let Some(session_id) = extract_session_id(body) {
        if headers.contains_key(&config.target_header) {
            tracing::debug!(
                upstream_id,
                header = %config.target_header,
                "header injection skipped: target header already present"
            );
            return None;
        }

        let Ok(value) = HeaderValue::from_str(session_id) else {
            tracing::warn!(
                upstream_id,
                "header injection skipped: session id is not a valid header value"
            );
            return None;
        };

        headers.insert(config.target_header.clone(), value);
        tracing::debug!(
            upstream_id,
            header = %config.target_header,
            session_id,
            "injected session header from hook marker"
        );
        return None;
    }

    if !config.fallback_enabled {
        tracing::debug!(
            upstream_id,
            "header injection enabled but no session marker found"
        );
        return None;
    }

    if headers.contains_key("x-session-id") || headers.contains_key("x-opencode-session") {
        tracing::debug!(
            upstream_id,
            "session fallback skipped: inbound session header already present"
        );
        return None;
    }

    let Some(store) = fallback_store else {
        tracing::debug!(upstream_id, "session fallback skipped: store unavailable");
        return None;
    };
    let Some(exchange) = fallback_extract::extract_first_exchange(body) else {
        tracing::debug!(
            upstream_id,
            "session fallback skipped: no first user/assistant exchange"
        );
        return None;
    };

    let user_hash = fallback_extract::user_hash(&exchange.user);
    let assistant_hash = exchange
        .assistant
        .as_deref()
        .map(fallback_extract::assistant_hash);
    let (id, capture) = match store.resolve(
        upstream_id,
        request_id,
        &user_hash,
        assistant_hash.as_deref(),
    ) {
        FallbackResolution::Existing(id) => (id, None),
        FallbackResolution::Pending { id, capture } => (id, Some(capture)),
    };

    if headers.contains_key(&config.target_header) {
        tracing::debug!(
            upstream_id,
            header = %config.target_header,
            "session fallback skipped: target header already present"
        );
        return None;
    }
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
