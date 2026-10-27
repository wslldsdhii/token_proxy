//! `/rate_*` 端点直调测试（不经 HTTP 层，直接构造 State 调用 handler）。

use super::*;

fn temp_state(tag: &str) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "token-proxy-rate-api-test-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let paths = TokenProxyPaths::from_app_data_dir(dir).expect("paths");
    let state = AppState {
        main: sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("main pool"),
        rate_db_path: paths.rate_db_path(),
    };
    (state, paths.rate_db_path())
}

fn rate_row(cid: Option<&str>, attempt: u64, output: u64) -> rate::RateEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    rate::RateEntry {
        client_request_id: cid.map(str::to_string),
        attempt_index: attempt,
        model: Some("glm-5.3".to_string()),
        provider: "glm-proxy".to_string(),
        upstream_id: "glm-proxy".to_string(),
        stream: true,
        status: 200,
        started_ts_ms: now,
        upstream_first_chunk_ts_ms: Some(now + 600),
        first_output_ts_ms: None,
        ended_ts_ms: now + 1_200,
        output_tokens: output,
        created_at_ms: now + 1_200,
    }
}

#[tokio::test]
async fn rate_endpoints_degrade_when_db_missing() {
    let (state, path) = temp_state("missing");
    assert!(!path.exists());

    let response = rate_max_id(State(state.clone())).await;
    let body = response.into_body();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.expect("body");
    assert_eq!(&bytes[..], br#"{"maxId":0}"#);

    let response = rate_rows(
        State(state),
        Query(RowsQuery {
            after_id: Some(0),
            limit: None,
        }),
    )
    .await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(&bytes[..], br#"{"maxId":0,"rows":[]}"#);
}

#[tokio::test]
async fn rate_rows_incremental_pull_with_camel_case() {
    let (state, path) = temp_state("incremental");
    let pool = rate::ensure_rate_pool(&path).await.expect("rate pool");
    rate::insert_rate_entry(&pool, &rate_row(Some("cid-1"), 0, 603))
        .await
        .expect("insert 1");
    rate::insert_rate_entry(&pool, &rate_row(Some("cid-2"), 0, 42))
        .await
        .expect("insert 2");

    let response = rate_rows(
        State(state.clone()),
        Query(RowsQuery {
            after_id: Some(0),
            limit: Some(1),
        }),
    )
    .await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let rows = payload["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "limit=1 只取首批");
    assert_eq!(rows[0]["clientRequestId"], "cid-1");
    assert_eq!(rows[0]["outputTokens"], 603);
    assert_eq!(rows[0]["startedTsMs"].as_i64().unwrap() + 600, rows[0]["upstreamFirstChunkTsMs"].as_i64().unwrap());
    assert!(rows[0]["firstOutputTsMs"].is_null());
    assert_eq!(rows[0]["stream"], true);
    assert!(payload["maxId"].as_i64().unwrap() >= 2, "maxId 恒为当前 MAX(id)");

    // 游标增量：afterId = 首行 id，拿到第二行。
    let first_id = rows[0]["id"].as_i64().unwrap();
    let response = rate_rows(
        State(state),
        Query(RowsQuery {
            after_id: Some(first_id),
            limit: Some(500),
        }),
    )
    .await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let rows = payload["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["clientRequestId"], "cid-2");

    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn rate_max_id_reports_watermark() {
    let (state, path) = temp_state("watermark");
    let pool = rate::ensure_rate_pool(&path).await.expect("rate pool");
    rate::insert_rate_entry(&pool, &rate_row(Some("cid-1"), 0, 1))
        .await
        .expect("insert");

    let response = rate_max_id(State(state)).await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert!(payload["maxId"].as_i64().unwrap() >= 1);

    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}
