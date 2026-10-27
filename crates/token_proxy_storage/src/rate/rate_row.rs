//! rate 行的只读查询模型（供 my_stats_api `/rate_*` 端点使用）。

use sqlx::sqlite::SqliteRow;
use sqlx::Row;

/// `rate_logs` 查询行；全部可空字段的口径与库内 DDL 一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateRow {
    pub id: i64,
    pub client_request_id: Option<String>,
    pub attempt_index: i64,
    pub model: Option<String>,
    pub provider: String,
    pub upstream_id: String,
    pub stream: bool,
    pub status: i64,
    pub started_ts_ms: i64,
    pub upstream_first_chunk_ts_ms: Option<i64>,
    pub first_output_ts_ms: Option<i64>,
    pub ended_ts_ms: i64,
    pub output_tokens: i64,
}

impl RateRow {
    pub fn from_row(row: SqliteRow) -> Option<Self> {
        Some(Self {
            id: row.try_get::<i64, _>("id").ok()?,
            client_request_id: row.try_get::<Option<String>, _>("client_request_id").ok()?,
            attempt_index: row.try_get::<i64, _>("attempt_index").unwrap_or(0),
            model: row.try_get::<Option<String>, _>("model").ok()?,
            provider: row.try_get::<String, _>("provider").ok()?,
            upstream_id: row.try_get::<String, _>("upstream_id").ok()?,
            stream: row.try_get::<i64, _>("stream").map(|v| v == 1).unwrap_or(true),
            status: row.try_get::<i64, _>("status").unwrap_or(0),
            started_ts_ms: row.try_get::<i64, _>("started_ts_ms").ok()?,
            upstream_first_chunk_ts_ms: row
                .try_get::<Option<i64>, _>("upstream_first_chunk_ts_ms")
                .ok()?,
            first_output_ts_ms: row.try_get::<Option<i64>, _>("first_output_ts_ms").ok()?,
            ended_ts_ms: row.try_get::<i64, _>("ended_ts_ms").ok()?,
            output_tokens: row.try_get::<i64, _>("output_tokens").unwrap_or(0),
        })
    }
}
