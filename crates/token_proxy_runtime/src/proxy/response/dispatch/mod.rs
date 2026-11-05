mod buffered;
mod stream;

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use std::sync::Arc;
use std::time::Duration;

use super::super::log::{LogContext, LogWriter};
use super::super::openai_compat::FormatTransform;
use super::super::session_fallback::SessionFallbackCapture;
use super::super::token_rate::RequestTokenTracker;
use token_proxy_protocol::xai_client_tools::XaiClientToolMapping;

pub(super) async fn build_stream_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    xai_client_tools: Option<XaiClientToolMapping>,
    fallback_capture: Option<SessionFallbackCapture>,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    response_format: Option<&str>,
    stream_first_output_timeout_remaining: Duration,
    stream_first_output_timeout: Duration,
    sync_response_timeout: Duration,
) -> Response {
    stream::build_stream_response(
        status,
        upstream_res,
        headers,
        context,
        log,
        request_tracker,
        response_transform,
        xai_client_tools,
        fallback_capture,
        model_override,
        estimated_input_tokens,
        response_format,
        stream_first_output_timeout_remaining,
        stream_first_output_timeout,
        sync_response_timeout,
    )
    .await
}

pub(super) async fn build_buffered_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    xai_client_tools: Option<XaiClientToolMapping>,
    fallback_capture: Option<SessionFallbackCapture>,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    response_format: Option<&str>,
    sync_response_timeout: Duration,
) -> Response {
    buffered::build_buffered_response(
        status,
        upstream_res,
        headers,
        context,
        log,
        request_tracker,
        response_transform,
        xai_client_tools,
        fallback_capture,
        model_override,
        estimated_input_tokens,
        response_format,
        sync_response_timeout,
    )
    .await
}

#[cfg(test)]
#[path = "buffered.test.rs"]
mod buffered_tests;
