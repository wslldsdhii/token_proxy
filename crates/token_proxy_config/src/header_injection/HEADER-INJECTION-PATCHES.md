# HEADER-INJECTION-PATCHES —— 上游更新后恢复渠道级请求头注入

对齐 `my_proxy_endpoint/MY-PROXY-ENDPOINT-PATCHES.md` 与 `rate_recorder/RATE-RECORDER-PATCHES.md` 的恢复惯例。
官方 main 不含本模块；每次 `git pull` 官方后需：检出 5 个自有目录/文件 → 重放 11 组 PATCH（config 9 处、runtime 2 处、CLI 5 处横幅、前端 10 处）→ 重放 i18n key 块 → 验证。

功能 = **5 个纯新增文件 + 11 组横幅注入（9 个上游文件）+ 2 组 i18n 文案块 + 3 处 GUI 操作优化**。

## 文件清单（纯新增，零上游冲突）

| 路径 | 内容 |
| --- | --- |
| `crates/token_proxy_config/src/header_injection/mod.rs` | 段结构、`de_lenient`、`resolve_channel`、`normalize_target_header`、`HeaderInjectionRuntime`、`enable_channel`/`disable_channel` |
| `crates/token_proxy_config/src/header_injection/mod.test.rs` | 段解析/归一化/非法值/版本忽略/开关语义 |
| `crates/token_proxy_config/src/header_injection/HEADER-INJECTION-PATCHES.md` | 本文件 |
| `crates/token_proxy_runtime/src/proxy/header_injection/mod.rs` | 标记扫描、`extract_session_id`、`apply` |
| `crates/token_proxy_runtime/src/proxy/header_injection/mod.test.rs` | 多格式/多标记/非法标记/不覆盖/幂等 |
| `src/features/config/header-injection.ts` | 表单纯函数：选项、读草稿、写记录、改名迁移、payload |
| `src/features/config/header-injection.test.ts` | 纯函数 + 编辑器保存语义单测 |

钩子模板不在本仓：上层工作区 `myplugins/trae-hooks/`（`hooks.json` + `hooks/user-prompt-submit.py` + `README.md`）。

## 依赖

零新增依赖。标记解析用 `str::find` 手写，不引入 `regex`。`src/features/config/header-injection.ts` 不依赖 React/i18n。

## 一、配置层（`crates/token_proxy_config`）

### PATCH 1 —— `src/lib.rs`（mod 声明）

锚点：`mod normalize;` 之上。

```rust
// ══════════ HEADER-INJECTION PATCH 1 (mod) START ══════════
pub mod header_injection;
// ══════════ HEADER-INJECTION PATCH 1 (mod) END ══════════
mod normalize;
```

### PATCH 2 —— `src/lib.rs`（build_runtime_config 传段）

锚点：`normalize::normalize_upstreams(` 调用内的 `&hot_model_mappings,` 之后。

```rust
    let normalized_upstreams = normalize::normalize_upstreams(
        &config.upstreams,
        app_proxy_url.as_deref(),
        &hot_model_mappings,
        // ══════════ HEADER-INJECTION PATCH 2 (normalize) START ══════════
        config.header_injection.as_ref(),
        // ══════════ HEADER-INJECTION PATCH 2 (normalize) END ══════════
    )?;
```

### PATCH 3 —— `src/types.rs`（三处，缺一编译失败）

3a `ProxyConfigFile` 字段：锚点 `MY-MODEL-MAPPING-PANEL PATCH 2 (field) END` 下的结构体末尾。

```rust
    // ══════════ HEADER-INJECTION PATCH 3 (field) START ══════════
    /// 本地增强：渠道级「会话标记 → 请求头注入」配置段（header_injection 模块）。
    #[serde(
        default,
        deserialize_with = "crate::header_injection::de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub header_injection: Option<crate::header_injection::HeaderInjectionSection>,
    // ══════════ HEADER-INJECTION PATCH 3 (field) END ══════════
```

3b `impl Default for ProxyConfigFile`：锚点 `MY-PROXY-ENDPOINT PATCH 2 (default) END` 之后。

```rust
            // ══════════ HEADER-INJECTION PATCH 3 (default) START ══════════
            header_injection: None,
            // ══════════ HEADER-INJECTION PATCH 3 (default) END ══════════
```

3c `UpstreamRuntime` 字段：锚点 `MY-URL-COMPOSE PATCH C2 END`（结构体末尾）。

```rust
    // ══════════ HEADER-INJECTION PATCH 3 (runtime field) START ══════════
    /// 本渠道的会话标记头注入配置（归一化后；None = 关闭）。
    pub header_injection: Option<crate::header_injection::HeaderInjectionRuntime>,
    // ══════════ HEADER-INJECTION PATCH 3 (runtime field) END ══════════
```

> `UpstreamRuntime` 无 `Default`：所有字面量构造点都会因缺字段在编译期报错，按报错逐个补 `header_injection: None,` 即可（生产 1 处、测试 9 处）。

### PATCH 4 —— `src/normalize.rs`（`normalize_upstreams` 参数与转发）

```rust
pub(super) fn normalize_upstreams(
    upstreams: &[UpstreamConfig],
    app_proxy_url: Option<&str>,
    hot_model_mappings: &HashMap<String, String>,
    // ══════════ HEADER-INJECTION PATCH 4 (param) START ══════════
    header_injection: Option<&crate::header_injection::HeaderInjectionSection>,
    // ══════════ HEADER-INJECTION PATCH 4 (param) END ══════════
) -> Result<Vec<NormalizedUpstream>, String> {
```

调用 `normalize_single_upstream(` 末尾追加：

```rust
            // ══════════ HEADER-INJECTION PATCH 4 (arg) START ══════════
            header_injection,
            // ══════════ HEADER-INJECTION PATCH 4 (arg) END ══════════
```

### PATCH 5 —— `src/normalize.rs`（`normalize_single_upstream`，三处）

5a 参数：

```rust
    // ══════════ HEADER-INJECTION PATCH 5 (param) START ══════════
    header_injection_section: Option<&crate::header_injection::HeaderInjectionSection>,
    // ══════════ HEADER-INJECTION PATCH 5 (param) END ══════════
```

5b `normalize_header_overrides(...)` 之后解析：

```rust
    // ══════════ HEADER-INJECTION PATCH 5 (resolve) START ══════════
    let header_injection =
        crate::header_injection::resolve_channel(header_injection_section, &upstream.id);
    // ══════════ HEADER-INJECTION PATCH 5 (resolve) END ══════════
```

5c `UpstreamRuntime { ... }` 字面量内（`url_compose` 之后）：

```rust
                // ══════════ HEADER-INJECTION PATCH 5 (attach) START ══════════
                header_injection: header_injection.clone(),
                // ══════════ HEADER-INJECTION PATCH 5 (attach) END ══════════
```

## 二、运行时层（`crates/token_proxy_runtime`）

### PATCH 6 —— `src/proxy/mod.rs`（模块声明）

锚点：`mod gemini_compat;` 与 `mod http;` 之间。

```rust
// ══════════ HEADER-INJECTION PATCH 6 (module) START ══════════
mod header_injection;
// ══════════ HEADER-INJECTION PATCH 6 (module) END ══════════
```

### PATCH 7 —— `src/proxy/upstream/prepare.rs`（注入调用）

锚点：`request::build_request_headers(...)` 之后、`if provider == "xai"` 之前。

```rust
    // ══════════ HEADER-INJECTION PATCH 7 (apply) START ══════════
    super::super::header_injection::apply(upstream, body, &mut request_headers);
    // ══════════ HEADER-INJECTION PATCH 7 (apply) END ══════════
```

## 三、CLI（`crates/token_proxy_cli/src/main.rs`）

### PATCH 8 —— `ConfigCommand` 变体 + `HeaderInjectionCommand` 枚举

在 `ConfigCommand` 的 `Init` 之后加：

```rust
    // ══════════ HEADER-INJECTION PATCH 8 (cli command) START ══════════
    /// 渠道级请求头注入配置（会话标记 → 请求头）
    HeaderInjection {
        #[command(subcommand)]
        command: HeaderInjectionCommand,
    },
    // ══════════ HEADER-INJECTION PATCH 8 (cli command) END ══════════
```

枚举定义（`ConfigCommand` 之后）：

```rust
// ══════════ HEADER-INJECTION PATCH 8 (cli subcommands) START ══════════
#[derive(Subcommand)]
enum HeaderInjectionCommand {
    List,
    Enable { #[arg(long)] channel: String, #[arg(long)] header: Option<String> },
    Disable { #[arg(long)] channel: String },
}
// ══════════ HEADER-INJECTION PATCH 8 (cli subcommands) END ══════════
```

### PATCH 9 —— `run()` 分发

`ConfigCommand::Init` 分支之后：

```rust
            // ══════════ HEADER-INJECTION PATCH 9 (cli dispatch) START ══════════
            ConfigCommand::HeaderInjection { command } => {
                run_header_injection(paths, command).await
            }
            // ══════════ HEADER-INJECTION PATCH 9 (cli dispatch) END ══════════
```

### PATCH 10 —— 实现函数（`run_header_injection` / `mutate_header_injection` / `print_header_injection_list` / `header_injection_rows`）

整块放在 `run_agent_node` 之前，横幅 `PATCH 10 (cli impl)`。

### PATCH 11 —— 单测（`mod tests` 内）

追加到既有 `mod tests`：clap 解析、enable/disable、非法 Header、孤儿行；横幅 `PATCH 11 (cli tests)`。

## 四、GUI 数据层

### PATCH G1 —— `src/features/config/types.ts`（三处）

- 配置段类型块（`HeaderOverrideForm` 之后，横幅 `PATCH G1`）；
- `ProxyConfigFileBase.header_injection?`（横幅 `PATCH G1 (payload)`）；
- `ConfigForm.headerInjection`（横幅 `PATCH G1 (form)`）。

### PATCH G2 —— `src/features/config/header-injection.ts`（整个新文件）

导出：`HEADER_INJECTION_VERSION` / `SESSION_RULE_ID` / `SESSION_RULE_SOURCE` / `DEFAULT_TARGET_HEADER` / `TARGET_HEADER_OPTIONS` / `normalizeHeaderInjectionSection` / `readChannelDraft` / `isChannelEnabled` / `applyChannelDraft` / `renameChannel` / `applyEditorSave` / `toHeaderInjectionPayload`。

### PATCH G3 —— `src/features/config/form.ts`（五处）

- imports；
- `KNOWN_CONFIG_KEYS` 增 `"header_injection"`；
- `EMPTY_FORM.headerInjection`；
- `toForm` 末尾 `headerInjection: normalizeHeaderInjectionSection(config.header_injection)`；
- `toPayload` 末尾条件展开 `header_injection`。

## 五、GUI 交互层

### PATCH G4 —— `src/features/config/cards/upstreams-card.tsx`

imports（`applyEditorSave` / `readChannelDraft` / 类型）、props（`headerInjection` + `onHeaderInjectionChange`）、三个打开函数注入草稿、`updateHeaderInjectionDraft`、`saveDraft` 调用 `applyEditorSave`、Dialog 透传。

### PATCH G5 —— `.../upstreams/editor-dialog.tsx`

props `headerInjection?` + `onHeaderInjectionChange`，渲染条件 `editor.open && headerInjection`，透传给 `UpstreamEditorFields`。

### PATCH G6 —— `.../upstreams/editor-dialog-form.tsx`

imports（`DEFAULT_TARGET_HEADER` / `TARGET_HEADER_OPTIONS` / 草稿类型）、props（可选，缺省关闭）、`UpstreamHeaderInjectionFields` 组件、`UpstreamAdvancedFields` 内挂在 `UpstreamHeaderOverrideFields` 之后。

### PATCH G7 —— `src/features/config/AppView.tsx`

props `onHeaderInjectionChange`，`UpstreamsCard` 传 `headerInjection` + `onHeaderInjectionChange`。

### PATCH G8 —— `src/features/config/ConfigScreen.tsx`

`onHeaderInjectionChange: (headerInjection) => state.updateForm({ headerInjection })`。

### PATCH G10 —— `.../upstreams/types.ts`

import 草稿类型；`UpstreamEditorState` 三个 open 变体携带 `headerInjection`。

## 六、i18n 文案块（`messages/zh.json` 与 `messages/en.json` 同步）

新增：

```text
upstreams_section_header_injection
upstreams_header_injection_enable
upstreams_header_injection_target
upstreams_header_injection_tip
upstreams_header_injection_option_session
upstreams_header_injection_option_opencode
upstreams_header_injection_hint
```

修改（GUI 操作优化 1）：

```text
my_proxy_endpoint_format_openai      → OpenAI
my_proxy_endpoint_format_anthropic   → Anthropic
my_proxy_endpoint_format_gemini      → Gemini
my_proxy_endpoint_no_suffix          → 删除（不再引用）
```

## 七、GUI 操作优化（三个既有自建组件）

1. `src/features/my-proxy-endpoint/MyProxyEndpointBar.tsx`：删除格式下拉项后缀 `<span>`（文件属自建 `my-proxy-endpoint`，无横幅）。
2. `src/features/my-model-mapping-panel/ModelMappingPanel.tsx`：标题行改单一 `<button>`；`sortableHeader` 与 `providers` 表头改「满格按钮 + 绝对定位拖拽手柄」（文件属自建 `my-model-mapping-panel`，无横幅）。
3. `src/features/config/cards/upstreams/available-models-editor.tsx`（官方文件）：`ScrollArea` 高度 `h-40` → `h-[28rem]` 并补 `data-testid`，横幅 `PATCH G9 (list height)`。

## 验证清单

```bash
cargo test -p token_proxy_config header_injection
cargo test -p token_proxy_runtime header_injection
cargo test -p token_proxy_cli
cargo test --workspace
./node_modules/.bin/tsc --noEmit
./node_modules/.bin/vitest run
./node_modules/.bin/paraglide-js compile --project ./project.inlang --outdir ./src/paraglide --strategy localStorage preferredLanguage baseLocale --emit-ts-declarations
```

> 本环境 `pnpm` 命令会挂起；直接调用 `node_modules/.bin/*` 等价。正常开发机仍用 `pnpm i18n:compile && pnpm test`。
