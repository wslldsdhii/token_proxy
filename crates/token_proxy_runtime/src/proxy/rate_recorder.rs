//! 请求级速率旁路记录器（runtime 侧全局挂靠，fork 本地增强）。
//!
//! `init` 只记录 rate.db 路径（不开池、不建文件——关闭态零文件）；
//! `maybe_record` 在 `build_log_entry` 尾部被调用：开关启用时惰性建池并
//! fire-and-forget 写一行**绝对时间点**快照（时长/速率派生留给消费侧）。
//! 初始化挂 CLI 与 GUI 共同经过的 `TokenProxyApp::open`（双端行为一致，
//! reload 重复 init 幂等）；写入失败仅 debug 日志，绝不影响转发热路径。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_storage::rate::{ensure_rate_pool, insert_rate_entry, RateEntry};

use super::log::{LogContext, LogEntry};

struct RateRecorderHandle {
    rate_db_path: PathBuf,
}

static HANDLE: OnceLock<std::sync::RwLock<Option<RateRecorderHandle>>> = OnceLock::new();

fn handle_cell() -> &'static std::sync::RwLock<Option<RateRecorderHandle>> {
    HANDLE.get_or_init(|| std::sync::RwLock::new(None))
}

/// CLI/GUI 共同装配点（`TokenProxyApp::open` 末尾）调用；重复调用为同值
/// 覆盖（幂等，无副作用——reload 不经过此点，生产每进程恰好一次）。
pub fn init(paths: &TokenProxyPaths) {
    if let Ok(mut slot) = handle_cell().write() {
        *slot = Some(RateRecorderHandle {
            rate_db_path: paths.rate_db_path(),
        });
    }
}

/// 供测试与诊断：当前已初始化的 rate.db 路径。
pub(crate) fn current_rate_db_path() -> Option<PathBuf> {
    handle_cell()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(|handle| handle.rate_db_path.clone()))
}

/// `build_log_entry` 尾部挂靠：开关未启用 / 未初始化时直接返回（零行为）；
/// 启用时派生绝对时间点并异步写入。
pub(crate) fn maybe_record(context: &LogContext, entry: &LogEntry) {
    let Some(resolved) = token_proxy_config::rate_recorder::current_settings() else {
        return;
    };
    if !resolved.enabled {
        return;
    }
    let Some(rate_db_path) = current_rate_db_path() else {
        return;
    };
    // 保留期在写入路径顺带下发（避免 config → storage 的直接依赖）。
    token_proxy_storage::rate::set_retention_days(i64::from(resolved.retention_days));

    let record = build_rate_entry(context, entry);
    tokio::spawn(async move {
        match ensure_rate_pool(&rate_db_path).await {
            Ok(pool) => {
                if let Err(err) = insert_rate_entry(&pool, &record).await {
                    tracing::debug!(error = %err, "rate db write failed");
                }
            }
            Err(err) => tracing::debug!(error = %err, "rate db open failed"),
        }
    });
}

/// 由请求结束快照派生速率行：`started = ended − start.elapsed()`（同刻派生，
/// 误差亚毫秒），偏移列换算为绝对时刻。纯函数便于单测。
pub(crate) fn build_rate_entry(context: &LogContext, entry: &LogEntry) -> RateEntry {
    let ended_ts_ms = entry.ts_ms;
    let started_ts_ms = ended_ts_ms.saturating_sub(context.start.elapsed().as_millis());
    let first_chunk_offset = entry
        .upstream_first_body_chunk_ms
        .or(entry.upstream_first_byte_ms)
        .or(context.ttfb_ms);
    RateEntry {
        client_request_id: entry.client_request_id.clone(),
        attempt_index: entry.attempt_index.unwrap_or(0),
        model: entry.model.clone(),
        provider: entry.provider.clone(),
        upstream_id: entry.upstream_id.clone(),
        stream: entry.stream,
        status: entry.status,
        started_ts_ms,
        upstream_first_chunk_ts_ms: first_chunk_offset.map(|offset| started_ts_ms + offset),
        first_output_ts_ms: entry.first_output_ms.map(|offset| started_ts_ms + offset),
        ended_ts_ms,
        output_tokens: entry
            .usage
            .as_ref()
            .and_then(|usage| usage.output_tokens)
            .unwrap_or(0),
        created_at_ms: now_wall_ms(),
    }
}

fn now_wall_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::log::{LogContext, LogEntry, TokenUsage};
    use std::time::{Duration, Instant};

    fn context(start_ago_ms: u128) -> LogContext {
        LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "up-a".to_string(),
            account_id: None,
            model: Some("alias".to_string()),
            mapped_model: Some("gpt-x".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: Some(400),
            timings: Default::default(),
            start: Instant::now() - Duration::from_millis(start_ago_ms as u64),
        }
    }

    fn entry_with(ts_ms: u128) -> LogEntry {
        LogEntry {
            ts_ms,
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "up-a".to_string(),
            account_id: None,
            model: Some("alias".to_string()),
            mapped_model: Some("gpt-x".to_string()),
            upstream_response_model: None,
            stream: true,
            status: 200,
            usage: Some(TokenUsage {
                input_tokens: Some(100),
                output_tokens: Some(603),
                total_tokens: Some(703),
            }),
            billable_usage: Default::default(),
            service_tier: None,
            usage_json: None,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            response_body: None,
            response_error: None,
            latency_ms: 0,
            upstream_first_byte_ms: Some(300),
            upstream_response_headers_ms: Some(250),
            upstream_first_body_chunk_ms: Some(400),
            first_client_flush_ms: Some(450),
            first_output_ms: Some(800),
            cost_nano_usd: None,
            pricing_version: String::new(),
            pricing_model: None,
            pricing_context_tier: None,
            client_request_id: Some("cid-1".to_string()),
            attempt_index: Some(0),
        }
    }

    #[test]
    fn build_rate_entry_derives_absolute_timestamps() {
        let started_ago = 3_000_u128;
        let record = build_rate_entry(&context(started_ago), &entry_with(now_wall_ms()));
        assert!(record.started_ts_ms <= record.ended_ts_ms);
        assert!(record.ended_ts_ms - record.started_ts_ms <= elapsed_upper_bound(started_ago));
        assert_eq!(
            record.upstream_first_chunk_ts_ms,
            Some(record.started_ts_ms + 400),
            "首块偏移换算为绝对时刻（body_chunk 优先）"
        );
        assert_eq!(
            record.first_output_ts_ms,
            Some(record.started_ts_ms + 800)
        );
        assert_eq!(record.output_tokens, 603, "usage 真值作分子");
        assert_eq!(record.client_request_id.as_deref(), Some("cid-1"));
    }

    #[test]
    fn build_rate_entry_falls_back_to_ttfb_for_first_chunk() {
        let mut source = entry_with(now_wall_ms());
        source.upstream_first_body_chunk_ms = None;
        source.upstream_first_byte_ms = None;
        let record = build_rate_entry(&context(1_000), &source);
        assert_eq!(
            record.upstream_first_chunk_ts_ms,
            Some(record.started_ts_ms + 400),
            "ttfb_ms 作为首块回退"
        );
    }

    #[test]
    fn build_rate_entry_without_usage_records_zero() {
        let mut source = entry_with(now_wall_ms());
        source.usage = None;
        let record = build_rate_entry(&context(1_000), &source);
        assert_eq!(record.output_tokens, 0, "无 usage 行记 0（错误行口径）");
        assert_eq!(record.attempt_index, 0, "无归因键时 attempt 回退 0");
    }

    fn elapsed_upper_bound(started_ago_ms: u128) -> u128 {
        started_ago_ms + 50
    }

    #[tokio::test]
    async fn maybe_record_disabled_keeps_no_rate_db_file() {
        let dir = std::env::temp_dir().join(format!(
            "token-proxy-rate-rec-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let paths = TokenProxyPaths::from_app_data_dir(dir.clone()).expect("paths");
        init(&paths);

        // 开关未启用（快照 None）：maybe_record 静默返回，不建文件。
        let ctx = context(1_000);
        maybe_record(&ctx, &entry_with(now_wall_ms()));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(!paths.rate_db_path().exists(), "未启用时不创建 rate.db");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
