//! my_stats_api —— 只读行级统计 HTTP API 守护（fork 本地增强）。
//!
//! 恢复指南见 `crates/token_proxy_config/src/my_stats_api/MY-STATS-API-PATCHES.md`。
//! 职责：在 CLI/GUI 汇聚的装配层（`TokenProxyApp::open` 末尾）启动一个独立线程 +
//! 自建 current_thread tokio runtime 的 axum 服务，暴露五个只读端点：
//! `GET /max_id`、`GET /rows?afterId&limit`、`GET /attribution?cid`，
//! 以及速率旁路库（rate.db）的 `GET /rate_max_id`、`GET /rate_rows?afterId&limit`
//! （RATE-RECORDER PATCH，camelCase 契约与既有端点一致）。
//! 设置与鉴权判定在 `token_proxy_config::my_stats_api`（token reload 即生效）；
//! 数据源为 `request_logs` 表本身（含错误行与 `is_billable=0` 行，过滤口径在消费方）。

use axum::{
    extract::{Request, Query, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::path::PathBuf;
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_config::my_stats_api::{auth_ok, current_settings};
use token_proxy_storage::rate::{self, RateRow};

const DEFAULT_ROW_LIMIT: i64 = 500;
const MAX_ROW_LIMIT: i64 = 2000;

/// PATCH 3 注入点：`TokenProxyApp::open` 末尾调用。
///
/// 使用 `std::thread::spawn` 独立线程 + 线程内自建 current_thread runtime——
/// `open` 是同步函数，GUI 侧在 tauri setup（无 tokio runtime 上下文）中调用，
/// `tokio::spawn` 会 panic；CLI 侧（tokio main）同样兼容。线程失败不 panic、
/// 不影响代理主功能。
pub fn spawn(paths: Arc<TokenProxyPaths>) {
    let spawned = std::thread::Builder::new()
        .name("my-stats-api".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(error = %err, "my-stats-api tokio runtime 创建失败，服务未启动");
                    return;
                }
            };
            runtime.block_on(serve(paths));
        });
    if spawned.is_err() {
        tracing::error!("my-stats-api 线程创建失败，服务未启动");
    }
}

async fn serve(paths: Arc<TokenProxyPaths>) {
    // open 返回时全局设置尚未同步（CLI 在 open 后才 read_config；GUI 更晚）——自行加载。
    let config = match token_proxy_config::read_config(&paths).await {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "my-stats-api 配置读取失败，服务未启动");
            return;
        }
    };
    let Some(settings) = token_proxy_config::my_stats_api::sync_settings(&config.config) else {
        tracing::debug!("my-stats-api 段缺失，服务未启动");
        return;
    };
    if !settings.enabled {
        tracing::debug!("my-stats-api 未启用（enabled=false），服务未启动");
        return;
    }
    let pool = match token_proxy_storage::sqlite::open_read_pool(&paths.sqlite_db_path()).await {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!(error = %err, "my-stats-api 读池打开失败，服务未启动");
            return;
        }
    };
    let state = AppState {
        main: pool,
        rate_db_path: paths.rate_db_path(),
    };
    let router = Router::new()
        .route("/max_id", get(max_id))
        .route("/rows", get(rows))
        .route("/attribution", get(attribution))
        // ══════════ RATE-RECORDER PATCH (routes) START ══════════
        .route("/rate_max_id", get(rate_max_id))
        .route("/rate_rows", get(rate_rows))
        // ══════════ RATE-RECORDER PATCH (routes) END ══════════
        .layer(middleware::from_fn(auth_middleware))
        .with_state(state);
    match tokio::net::TcpListener::bind((settings.host.as_str(), settings.port)).await {
        Ok(listener) => {
            tracing::info!(addr = %settings.host, port = settings.port, "my-stats-api 已启动");
            if let Err(err) = axum::serve(listener, router).await {
                tracing::error!(error = %err, "my-stats-api 服务异常退出");
            }
        }
        Err(err) => tracing::error!(error = %err, "my-stats-api 端口绑定失败，服务未启动"),
    }
}

async fn auth_middleware(req: Request, next: Next) -> Response {
    let settings = current_settings().unwrap_or_default();
    let provided = bearer_token(&req).or_else(|| query_token(&req));
    if !auth_ok(&settings, provided.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

fn bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

/// `?token=<value>` 兼容形式（注意会进访问日志，优先使用 Bearer 头）。
fn query_token(req: &Request) -> Option<String> {
    req.uri().query().and_then(|query| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "token").then(|| value.to_string())
        })
    })
}

/// 路由状态：主库只读池 + rate.db 路径（rate 端点惰性开池，未启用时降级空响应）。
#[derive(Clone)]
struct AppState {
    main: sqlx::SqlitePool,
    rate_db_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowsQuery {
    after_id: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct AttributionQuery {
    cid: String,
}

/// 行结构对齐统计侧行映射需求；全部 token 分量与归因列按 `Option` 建模
/// （旧行 / 未回填行为 NULL，消费方按 0 计）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsRow {
    id: i64,
    ts_ms: i64,
    provider: String,
    upstream_id: String,
    model: Option<String>,
    mapped_model: Option<String>,
    status: i64,
    is_billable: bool,
    client_request_id: Option<String>,
    attempt_index: Option<i64>,
    uncached_input_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cache_write_5m_tokens: Option<i64>,
    cache_write_1h_tokens: Option<i64>,
    image_input_tokens: Option<i64>,
    image_output_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

fn map_row(row: sqlx::sqlite::SqliteRow) -> Option<StatsRow> {
    use sqlx::Row;
    Some(StatsRow {
        id: row.try_get::<i64, _>("id").ok()?,
        ts_ms: row.try_get::<i64, _>("ts_ms").ok()?,
        provider: row.try_get::<String, _>("provider").ok()?,
        upstream_id: row.try_get::<String, _>("upstream_id").ok()?,
        model: row.try_get::<Option<String>, _>("model").ok()?,
        mapped_model: row.try_get::<Option<String>, _>("mapped_model").ok()?,
        status: row.try_get::<i64, _>("status").unwrap_or(0),
        is_billable: row.try_get::<i64, _>("is_billable").map(|v| v == 1).unwrap_or(false),
        client_request_id: row.try_get::<Option<String>, _>("client_request_id").ok()?,
        attempt_index: row.try_get::<Option<i64>, _>("attempt_index").ok()?,
        uncached_input_tokens: row.try_get::<Option<i64>, _>("uncached_input_tokens").ok()?,
        cache_read_tokens: row.try_get::<Option<i64>, _>("cache_read_tokens").ok()?,
        cache_write_tokens: row.try_get::<Option<i64>, _>("cache_write_tokens").ok()?,
        cache_write_5m_tokens: row.try_get::<Option<i64>, _>("cache_write_5m_tokens").ok()?,
        cache_write_1h_tokens: row.try_get::<Option<i64>, _>("cache_write_1h_tokens").ok()?,
        image_input_tokens: row.try_get::<Option<i64>, _>("image_input_tokens").ok()?,
        image_output_tokens: row.try_get::<Option<i64>, _>("image_output_tokens").ok()?,
        output_tokens: row.try_get::<Option<i64>, _>("output_tokens").ok()?,
    })
}

async fn max_id(State(state): State<AppState>) -> Response {
    let pool = state.main;
    match sqlx::query("SELECT COALESCE(MAX(id), 0) AS max_id FROM request_logs")
        .fetch_one(&pool)
        .await
    {
        Ok(row) => {
            let max_id: i64 = sqlx::Row::try_get(&row, "max_id").unwrap_or(0);
            Json(json!({ "maxId": max_id })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

async fn rows(
    State(state): State<AppState>,
    Query(query): Query<RowsQuery>,
) -> Response {
    let pool = state.main;
    let limit = query.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROW_LIMIT);
    let after_id = query.after_id.unwrap_or(0);
    let list = sqlx::query(
        "SELECT id, ts_ms, provider, upstream_id, model, mapped_model, status, is_billable, \
         client_request_id, attempt_index, uncached_input_tokens, cache_read_tokens, \
         cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, image_input_tokens, \
         image_output_tokens, output_tokens \
         FROM request_logs WHERE id > ?1 ORDER BY id LIMIT ?2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(&pool)
    .await;
    match list {
        Ok(list) => {
            let rows: Vec<StatsRow> = list.into_iter().filter_map(map_row).collect();
            // maxId 恒为源表当前 MAX(id)（与本批次行数无关），消费方拉空即知已追平。
            let max_id = sqlx::query("SELECT COALESCE(MAX(id), 0) AS max_id FROM request_logs")
                .fetch_one(&pool)
                .await
                .ok()
                .and_then(|row| sqlx::Row::try_get::<i64, _>(&row, "max_id").ok())
                .unwrap_or(0);
            Json(json!({ "maxId": max_id, "rows": rows })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

async fn attribution(
    State(state): State<AppState>,
    Query(query): Query<AttributionQuery>,
) -> Response {
    let pool = state.main;
    let row = sqlx::query(
        "SELECT id, ts_ms, provider, upstream_id, model, mapped_model, status, is_billable, \
         client_request_id, attempt_index, uncached_input_tokens, cache_read_tokens, \
         cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, image_input_tokens, \
         image_output_tokens, output_tokens \
         FROM request_logs WHERE client_request_id = ?1 \
         ORDER BY attempt_index DESC, id DESC LIMIT 1",
    )
    .bind(&query.cid)
    .fetch_optional(&pool)
    .await;
    match row {
        Ok(row) => {
            let row = row.and_then(map_row);
            Json(json!({ "row": row })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

fn internal_error(err: sqlx::Error) -> Response {
    tracing::error!(error = %err, "my-stats-api 查询失败");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "query failed"}))).into_response()
}

// ══════════ RATE-RECORDER PATCH (rate endpoints) START ══════════

/// `/rate_rows` 响应行（camelCase，契约对齐既有 `/rows`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RateStatsRow {
    id: i64,
    client_request_id: Option<String>,
    attempt_index: i64,
    model: Option<String>,
    provider: String,
    upstream_id: String,
    stream: bool,
    status: i64,
    started_ts_ms: i64,
    upstream_first_chunk_ts_ms: Option<i64>,
    first_output_ts_ms: Option<i64>,
    ended_ts_ms: i64,
    output_tokens: i64,
}

impl From<RateRow> for RateStatsRow {
    fn from(row: RateRow) -> Self {
        Self {
            id: row.id,
            client_request_id: row.client_request_id,
            attempt_index: row.attempt_index,
            model: row.model,
            provider: row.provider,
            upstream_id: row.upstream_id,
            stream: row.stream,
            status: row.status,
            started_ts_ms: row.started_ts_ms,
            upstream_first_chunk_ts_ms: row.upstream_first_chunk_ts_ms,
            first_output_ts_ms: row.first_output_ts_ms,
            ended_ts_ms: row.ended_ts_ms,
            output_tokens: row.output_tokens,
        }
    }
}

/// rate.db 只读池；库文件不存在（记录从未启用）时返回 None → 端点降级空响应。
async fn rate_pool_if_any(state: &AppState) -> Option<sqlx::SqlitePool> {
    rate::open_rate_read_pool_if_exists(&state.rate_db_path).await
}

async fn rate_max_id(State(state): State<AppState>) -> Response {
    let Some(pool) = rate_pool_if_any(&state).await else {
        return Json(json!({ "maxId": 0 })).into_response();
    };
    match rate::max_rate_id(&pool).await {
        Ok(max_id) => Json(json!({ "maxId": max_id })).into_response(),
        Err(err) => internal_error(err),
    }
}

async fn rate_rows(
    State(state): State<AppState>,
    Query(query): Query<RowsQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROW_LIMIT);
    let after_id = query.after_id.unwrap_or(0);
    let Some(pool) = rate_pool_if_any(&state).await else {
        return Json(json!({ "maxId": 0, "rows": [] })).into_response();
    };
    match rate::select_rate_rows_after(&pool, after_id, limit).await {
        Ok(rows) => {
            let payload: Vec<RateStatsRow> = rows.into_iter().map(Into::into).collect();
            // maxId 恒为源表当前 MAX(id)（与本批次行数无关），消费方拉空即知已追平。
            let max_id = rate::max_rate_id(&pool).await.unwrap_or(0);
            Json(json!({ "maxId": max_id, "rows": payload })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

// ══════════ RATE-RECORDER PATCH (rate endpoints) END ══════════

#[cfg(test)]
#[path = "rate_endpoints.test.rs"]
mod rate_endpoints_tests;
