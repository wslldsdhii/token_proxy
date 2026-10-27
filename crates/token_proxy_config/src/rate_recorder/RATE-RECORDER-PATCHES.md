# RATE-RECORDER PATCHES —— 恢复指南（fork 本地增强）

> 官方 main 同步后重放本表即可恢复全部功能；锚点以本文件为准，**不要凭记忆重写**。
> 行为契约：`openspec/specs/rate-recorder/spec.md`（归档后）；设计决策见 change `add-rate-recorder/design.md`。
> 配套面板（GUI）：`my-model-mapping-panel` 注入点也在本表末段。

## 一、官方文件内注入点（横幅标记逐字重放）

| # | 横幅 | 文件 | 改动 |
|---|---|---|---|
| 1 | `RATE-RECORDER PATCH 0 (module)` | `crates/token_proxy_storage/src/lib.rs` | `pub mod rate;`（在 `pub mod logs;` 之后） |
| 2 | `RATE-RECORDER PATCH 1 (path)` | `crates/token_proxy_account_store/src/paths.rs` | `RATE_DB_FILE_NAME: &str = "rate.db"` 常量 + `rate_db_path()` 方法（照 `sqlite_db_path`） |
| 3 | `RATE-RECORDER PATCH 1 (mod)` | `crates/token_proxy_config/src/lib.rs` | `pub mod rate_recorder;` |
| 4 | `RATE-RECORDER PATCH 2 (sync)` | 同上文件 `build_runtime_config` 内 | `let _ = rate_recorder::sync_settings(&config);`（紧跟 MY-STATS-API PATCH 1 之后） |
| 5 | `RATE-RECORDER PATCH 3 (field)` | `crates/token_proxy_config/src/types.rs` | `ProxyConfigFile` 加 `rate_recorder: Option<crate::rate_recorder::RateRecorderSection>`（serde default + skip）+ `Default` 实现补 `rate_recorder: None` |
| 6 | `RATE-RECORDER PATCH (module)` | `crates/token_proxy_runtime/src/proxy/mod.rs` | `pub mod rate_recorder;`（`mod redact;` 之前，字母序） |
| 7 | `RATE-RECORDER PATCH 1 (bypass record)` | `crates/token_proxy_runtime/src/proxy/log.rs` `build_log_entry` 尾部 | `LogEntry {...}` 字面量改为 `let entry = LogEntry {...};` + 调用 `crate::proxy::rate_recorder::maybe_record(context, &entry);` + 尾表达式 `entry` |
| 8 | `RATE-RECORDER PATCH 4 (init)` | `crates/token_proxy_app/src/app.rs` `TokenProxyApp::open` 末尾 | `token_proxy_runtime::proxy::rate_recorder::init(&paths);`（紧跟 MY-STATS-API PATCH 3 之后；CLI/GUI 唯一共同装配点） |
| 9 | `RATE-RECORDER PATCH (routes)` | `crates/token_proxy_app/src/my_stats_api/mod.rs`（**fork 独立文件**，非官方代码） | Router 加 `/rate_max_id`、`/rate_rows` 两路由；state 改 `AppState { main, rate_db_path }` |
| 10 | `MY-MODEL-MAPPING-PANEL PATCH 1 (mod)` | `crates/token_proxy_config/src/lib.rs` | `pub mod my_model_mapping_panel;` |
| 11 | `MY-MODEL-MAPPING-PANEL PATCH 2 (field)` | `crates/token_proxy_config/src/types.rs` | `my_model_mapping_panel: Option<...>`（de_lenient 宽容解析）+ Default 补 None |
| 12 | `MY-MODEL-MAPPING-PANEL PATCH 1 (mod)` | `src-tauri/src/lib.rs` | `mod my_model_mapping_panel;` |
| 13 | `MY-MODEL-MAPPING-PANEL PATCH 2 (commands)` | 同上 `invoke_handler` 列表 | `model_mapping_panel_data`、`model_mapping_panel_set_columns` |
| 14 | `MY-MODEL-MAPPING-PANEL PATCH G0 (import)` | `src/features/dashboard/DashboardPanel.tsx` | `lazy` 导入 `ModelMappingPanel` |
| 15 | `MY-MODEL-MAPPING-PANEL PATCH G1 (mount)` | 同上 | `<MyProxyEndpointBar />` 之后 `<Suspense fallback={null}><ModelMappingPanel /></Suspense>` |

## 二、独立新增文件（官方同步时整目录搬回）

| 文件 | 职责 |
|---|---|
| `crates/token_proxy_storage/src/rate.rs` + `rate/rate_row.rs` + `rate/tests.rs` | rate.db 三件套：独立建池（WAL）/ 建表+唯一索引 / INSERT OR IGNORE / 独立 retention 循环 / 查询行模型 |
| `crates/token_proxy_config/src/rate_recorder/mod.rs` + `mod.test.rs` | 配置段（enabled/retention_days）+ 全局快照 sync/current |
| `crates/token_proxy_config/src/my_model_mapping_panel/mod.rs` + `mod.test.rs` | 面板列显示持久化段 + 容错反序列化 |
| `crates/token_proxy_runtime/src/proxy/rate_recorder.rs` | 全局 handle + `init`/`maybe_record`/`build_rate_entry`（含单测） |
| `crates/token_proxy_app/src/my_stats_api/rate_endpoints.test.rs` | `/rate_*` 端点直调测试 |
| `src-tauri/src/my_model_mapping_panel.rs` | 面板数据/列持久化两个 tauri command（含单测） |
| `src/features/my-model-mapping-panel/`（api/types/logic/组件/两个测试） | 仪表盘模型映射面板（纯函数逻辑 + UI） |
| `messages/en.json`、`messages/zh.json` | `my_model_mapping_panel_*` 键 23 条（en/zh 对齐），改后 `pnpm i18n:compile` |

## 三、恢复步骤（官方 main 同步后）

1. 检出本功能分支旧版，把第二节独立文件整目录拷回；
2. 按第一节表格逐条重放横幅注入点（`build_log_entry` 尾部注入注意官方可能加列——注入形态不变，只是字面量更长）；
3. `cargo test --workspace` + `pnpm i18n:compile` + `pnpm test` + `pnpm build`；
4. 实机冒烟：`rate_recorder.enabled=true` 打请求 → `rate.db` 行数与 `data.db` terminal 行数一致；`curl :port+100/rate_max_id`。

## 四、关键设计约束（重放时勿"顺手优化"）

- rate.db **禁止**复用 `sqlite::open_read_pool/open_write_pool`（绑定主库 init_schema + 7 天 retention 副作用）；
- `client_request_id` 列**允许 NULL**（主库语义 Option；无归因键行是无归因速率样本，非错误）；
- `(client_request_id, attempt_index)` 唯一索引 + `INSERT OR IGNORE`（write-once 库级幂等；SQLite 中 NULL 不参与唯一冲突）；
- `started_ts_ms = ended − start.elapsed()` 同刻派生（勿改结构体加字段）；
- 关闭开关时**不得创建 rate.db 文件**（池惰性建立）；
- 面板展开高度常量 `EXPANDED_BODY_HEIGHT_PX = 520`（默认窗口 1064×658 推导；官方改默认窗口尺寸时在同步核对表复查）；
- `/rate_*` 响应 camelCase（与 `/rows` 一致），limit clamp 1..2000 默认 500。
