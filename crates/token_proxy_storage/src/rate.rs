//! rate —— 请求级速率旁路记录库（rate.db，fork 本地增强）。
//!
//! 独立 SQLite 库：独立连接（**不经过 `sqlite::open_*`**——后者绑定主库
//! `init_schema` 与 7 天 retention 副作用）、独立保留期（默认 90 天，
//! config 同步可更新）、write-once（`(client_request_id, attempt_index)`
//! 唯一索引 + INSERT OR IGNORE 吸收重复；NULL cid 不参与唯一约束）。
//! 只存绝对时间点与 usage 真值，时长/速率派生一律留给消费侧。
//! 行为契约：openspec specs `rate-recorder`。

use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OnceCell};

mod rate_row;

pub use rate_row::RateRow;

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
pub const DEFAULT_RETENTION_DAYS: i64 = 90;
const RETENTION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 当前生效保留期（天）；由 config 同步（reload 可更新），清理循环每轮读取。
static RETENTION_DAYS: AtomicI64 = AtomicI64::new(DEFAULT_RETENTION_DAYS);

/// 进程内单池缓存（rate.db 只有一个，按路径一致性校验后顶替重建）。
static RATE_POOL: OnceCell<Mutex<Option<(PathBuf, SqlitePool)>>> = OnceCell::const_new();

pub fn set_retention_days(days: i64) {
    if days > 0 {
        RETENTION_DAYS.store(days, Ordering::Relaxed);
    }
}

/// 旁路速率行（绝对墙钟时间点，单位 ms）。
#[derive(Debug, Clone)]
pub struct RateEntry {
    pub client_request_id: Option<String>,
    pub attempt_index: u64,
    pub model: Option<String>,
    pub provider: String,
    pub upstream_id: String,
    pub stream: bool,
    pub status: u16,
    pub started_ts_ms: u128,
    pub upstream_first_chunk_ts_ms: Option<u128>,
    pub first_output_ts_ms: Option<u128>,
    pub ended_ts_ms: u128,
    pub output_tokens: u64,
    pub created_at_ms: u128,
}

/// 打开（或复用）rate.db 连接池；首次打开建表并启动独立保留期清理循环。
/// 建池即建文件——仅应在记录开关启用后调用（关闭态零文件由调用方保证）。
pub async fn ensure_rate_pool(db_path: &Path) -> Result<SqlitePool, String> {
    let cell = RATE_POOL
        .get_or_init(|| async { Mutex::new(None) })
        .await;
    let mut guard = cell.lock().await;
    if let Some((cached, pool)) = guard.as_ref() {
        if cached == db_path {
            return Ok(pool.clone());
        }
    }
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| format!("Failed to create rate db directory: {err}"))?;
    }
    let pool = connect_rate_pool(db_path).await?;
    init_rate_schema(&pool).await?;
    spawn_rate_retention(pool.clone(), db_path.to_path_buf());
    *guard = Some((db_path.to_path_buf(), pool.clone()));
    Ok(pool)
}

/// 只读查询池：库文件不存在时返回 `None`（不创建文件——未启用记录的降级语义）。
pub async fn open_rate_read_pool_if_exists(db_path: &Path) -> Option<SqlitePool> {
    if !db_path.exists() {
        return None;
    }
    ensure_rate_pool(db_path).await.ok()
}

pub async fn insert_rate_entry(pool: &SqlitePool, entry: &RateEntry) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
INSERT OR IGNORE INTO rate_logs (
  client_request_id, attempt_index, model, provider, upstream_id,
  stream, status, started_ts_ms, upstream_first_chunk_ts_ms, first_output_ts_ms,
  ended_ts_ms, output_tokens, created_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
    )
    .bind(entry.client_request_id.as_deref())
    .bind(entry.attempt_index as i64)
    .bind(entry.model.as_deref())
    .bind(entry.provider.as_str())
    .bind(entry.upstream_id.as_str())
    .bind(i64::from(entry.stream))
    .bind(i64::from(entry.status))
    .bind(to_i64_u128(entry.started_ts_ms))
    .bind(entry.upstream_first_chunk_ts_ms.map(to_i64_u128))
    .bind(entry.first_output_ts_ms.map(to_i64_u128))
    .bind(to_i64_u128(entry.ended_ts_ms))
    .bind(entry.output_tokens as i64)
    .bind(to_i64_u128(entry.created_at_ms))
    .execute(pool)
    .await
    .map(|_| ())
}

/// 增量拉取：`id > after_id` 按 id 升序，`limit` 由调用方 clamp。
pub async fn select_rate_rows_after(
    pool: &SqlitePool,
    after_id: i64,
    limit: i64,
) -> Result<Vec<RateRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, client_request_id, attempt_index, model, provider, upstream_id, \
         stream, status, started_ts_ms, upstream_first_chunk_ts_ms, first_output_ts_ms, \
         ended_ts_ms, output_tokens FROM rate_logs WHERE id > ? ORDER BY id ASC LIMIT ?;",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().filter_map(RateRow::from_row).collect())
}

pub async fn max_rate_id(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COALESCE(MAX(id), 0) AS max_id FROM rate_logs;")
        .fetch_one(pool)
        .await?;
    use sqlx::Row;
    Ok(row.try_get::<i64, _>("max_id").unwrap_or(0))
}

fn spawn_rate_retention(pool: SqlitePool, db_path: PathBuf) {
    tokio::spawn(async move {
        loop {
            let retention_days = RETENTION_DAYS.load(Ordering::Relaxed);
            match retain_rate_logs(&pool, current_time_ms(), retention_days).await {
                Ok(deleted) if deleted > 0 => tracing::info!(
                    database = %db_path.display(),
                    deleted_rate_rows = deleted,
                    retention_days,
                    "rate log retention completed"
                ),
                Ok(_) => {}
                Err(error) => tracing::error!(
                    database = %db_path.display(),
                    error = %error,
                    "rate log retention failed"
                ),
            }
            tokio::time::sleep(RETENTION_INTERVAL).await;
        }
    });
}

async fn retain_rate_logs(
    pool: &SqlitePool,
    now_ms: i64,
    retention_days: i64,
) -> Result<u64, String> {
    let cutoff_ms = now_ms.saturating_sub(retention_days.saturating_mul(DAY_MS));
    sqlx::query("DELETE FROM rate_logs WHERE created_at_ms < ?;")
        .bind(cutoff_ms)
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
        .map_err(|error| format!("Failed to delete expired rate rows: {error}"))
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn to_i64_u128(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

async fn connect_rate_pool(path: &Path) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|err| format!("Failed to connect rate db: {err}"))
}

pub async fn init_rate_schema(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS rate_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  client_request_id TEXT,
  attempt_index INTEGER NOT NULL DEFAULT 0,
  model TEXT,
  provider TEXT,
  upstream_id TEXT,
  stream INTEGER NOT NULL DEFAULT 1,
  status INTEGER NOT NULL,
  started_ts_ms INTEGER NOT NULL,
  upstream_first_chunk_ts_ms INTEGER,
  first_output_ts_ms INTEGER,
  ended_ts_ms INTEGER NOT NULL,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create rate_logs table: {err}"))?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_rate_logs_cid_attempt \
         ON rate_logs (client_request_id, attempt_index);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_rate_logs_cid_attempt: {err}"))?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_rate_logs_created_at ON rate_logs(created_at_ms);")
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to create idx_rate_logs_created_at: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
