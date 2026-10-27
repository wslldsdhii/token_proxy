use super::*;
use sqlx::sqlite::SqlitePoolOptions;

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn entry(cid: Option<&str>, attempt: u64, offset_ms: u128) -> RateEntry {
    let base = now_ms();
    RateEntry {
        client_request_id: cid.map(str::to_string),
        attempt_index: attempt,
        model: Some("alias-model".to_string()),
        provider: "openai".to_string(),
        upstream_id: "up-a".to_string(),
        stream: true,
        status: 200,
        started_ts_ms: base + offset_ms,
        upstream_first_chunk_ts_ms: Some(base + offset_ms + 500),
        first_output_ts_ms: Some(base + offset_ms + 800),
        ended_ts_ms: base + offset_ms + 2_000,
        output_tokens: 603,
        created_at_ms: base + offset_ms + 2_000,
    }
}

fn temp_rate_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("token-proxy-rate-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("rate.db")
}

#[tokio::test]
async fn rate_schema_inserts_and_cursor_queries() {
    let path = temp_rate_path("cursor");
    let pool = ensure_rate_pool(&path).await.expect("open rate pool");

    insert_rate_entry(&pool, &entry(Some("cid-1"), 0, 1_000))
        .await
        .expect("insert first");
    insert_rate_entry(&pool, &entry(Some("cid-1"), 1, 5_000))
        .await
        .expect("insert second");
    insert_rate_entry(&pool, &entry(Some("cid-2"), 0, 9_000))
        .await
        .expect("insert third");

    let rows = select_rate_rows_after(&pool, 0, 10).await.expect("select all");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].client_request_id.as_deref(), Some("cid-1"));
    assert_eq!(rows[0].attempt_index, 0);
    assert_eq!(rows[1].attempt_index, 1);
    assert_eq!(
        rows[0].upstream_first_chunk_ts_ms,
        Some(rows[0].started_ts_ms + 500)
    );
    assert_eq!(rows[0].ended_ts_ms, rows[0].started_ts_ms + 2_000);

    let tail = select_rate_rows_after(&pool, rows[1].id, 10)
        .await
        .expect("select tail");
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].client_request_id.as_deref(), Some("cid-2"));

    assert_eq!(max_rate_id(&pool).await.expect("max id"), rows[2].id);
}

#[tokio::test]
async fn duplicate_key_is_absorbed_by_unique_index() {
    let path = temp_rate_path("dedupe");
    let pool = ensure_rate_pool(&path).await.expect("open rate pool");

    let first = entry(Some("cid-x"), 0, 1_000);
    let first_started = first.started_ts_ms as i64;
    insert_rate_entry(&pool, &first).await.expect("insert first");
    // 同 (cid, attempt) 二次提交：静默丢弃，不报错、不增行。
    insert_rate_entry(&pool, &entry(Some("cid-x"), 0, 2_000))
        .await
        .expect("duplicate insert ignored");

    let rows = select_rate_rows_after(&pool, 0, 10).await.expect("select");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].started_ts_ms, first_started, "保留首行");

    // NULL cid 不参与唯一约束：两行共存（无归因键的速率样本）。
    insert_rate_entry(&pool, &entry(None, 0, 3_000))
        .await
        .expect("insert null cid first");
    insert_rate_entry(&pool, &entry(None, 0, 4_000))
        .await
        .expect("insert null cid second");
    let rows = select_rate_rows_after(&pool, 0, 10).await.expect("select");
    assert_eq!(rows.len(), 3, "NULL cid 各自独立成行");
}

#[tokio::test]
async fn retention_deletes_only_expired_rows() {
    // 直连内存库（不经 ensure_rate_pool，避开真实时间基准的清理循环），
    // 用受控 now 显式驱动保留期判定。
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect memory sqlite");
    init_rate_schema(&pool).await.expect("init rate schema");

    let now = 10_000_000_000_i64;
    let day_ms = DAY_MS;
    let controlled_entry = |cid: &str, created_at_ms: i64| RateEntry {
        client_request_id: Some(cid.to_string()),
        attempt_index: 0,
        model: None,
        provider: "openai".to_string(),
        upstream_id: "up-a".to_string(),
        stream: true,
        status: 200,
        started_ts_ms: created_at_ms as u128,
        upstream_first_chunk_ts_ms: None,
        first_output_ts_ms: None,
        ended_ts_ms: created_at_ms as u128,
        output_tokens: 0,
        created_at_ms: created_at_ms as u128,
    };
    insert_rate_entry(&pool, &controlled_entry("old", now - 91 * day_ms))
        .await
        .expect("insert old");
    insert_rate_entry(&pool, &controlled_entry("new", now - 10 * day_ms))
        .await
        .expect("insert new");

    let deleted = retain_rate_logs(&pool, now, 90).await.expect("retain");
    assert_eq!(deleted, 1);
    let rows = select_rate_rows_after(&pool, 0, 10).await.expect("select");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].client_request_id.as_deref(), Some("new"));
}

#[tokio::test]
async fn rate_schema_does_not_create_main_tables() {
    let path = temp_rate_path("isolation");
    let pool = ensure_rate_pool(&path).await.expect("open rate pool");

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%';",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables");
    assert_eq!(tables, vec!["rate_logs".to_string()], "不建主库表");
}

#[tokio::test]
async fn read_pool_if_exists_degrades_without_file() {
    let path = temp_rate_path("missing");
    // 未建库文件时返回 None，且不创建文件。
    assert!(open_rate_read_pool_if_exists(&path).await.is_none());
    assert!(!path.exists());
}
