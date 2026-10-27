//! rate_recorder —— 请求级速率旁路记录的配置段（fork 本地增强）。
//!
//! 与 `my_stats_api` 同一 fork 补丁惯例：所有新增代码都在本目录，官方代码
//! 仅保留带横幅标记的注入点；恢复指南见仓库根 `crates/token_proxy_storage/src/rate.rs`
//! 头注释与 `docs/项目维护/261027-01-rate-recorder速率旁路统计/`。
//!
//! 本文件职责：`config.jsonc` 顶层 `rate_recorder` 段的解析结构、
//! 解析快照的全局同步（reload 即生效开关）、保留期下发。

use crate::ProxyConfigFile;
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

/// `config.jsonc` 顶层 `rate_recorder` 段的原始形态（全部字段可选）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateRecorderSection {
    /// 缺省 false：不启用旁路记录，且不创建 rate.db 文件。
    pub enabled: Option<bool>,
    /// 独立保留期（天）；缺省 90。
    pub retention_days: Option<u32>,
}

/// 解析后的生效快照（缺省值已填充）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateRecorderResolved {
    pub enabled: bool,
    pub retention_days: u32,
}

pub const DEFAULT_RATE_RETENTION_DAYS: u32 = 90;

/// 段 → 生效快照：`enabled` 缺省 false；`retention_days` 缺省 90（0 视为缺省）。
pub fn resolve_section(section: &RateRecorderSection) -> RateRecorderResolved {
    RateRecorderResolved {
        enabled: section.enabled.unwrap_or(false),
        retention_days: section
            .retention_days
            .filter(|days| *days > 0)
            .unwrap_or(DEFAULT_RATE_RETENTION_DAYS),
    }
}

/// 全局生效快照：`OnceLock` 承载 `RwLock` 外壳，内层 `RwLock` 保证每次
/// `sync_settings`（配置加载 / reload）真正覆盖旧值——开关 reload 即生效。
static RESOLVED: OnceLock<RwLock<Option<RateRecorderResolved>>> = OnceLock::new();

fn settings_cell() -> &'static RwLock<Option<RateRecorderResolved>> {
    RESOLVED.get_or_init(|| RwLock::new(None))
}

/// PATCH 注入点：`build_runtime_config` 中调用（每次配置加载 / reload 都会走到）。
///
/// 同步生效快照到全局；写入方（runtime 旁路挂靠）每请求读取，
/// 因此 **enabled / retention_days 变更 reload 即生效**（保留期由写入方
/// 顺带下发给存储层，避免 config → storage 的反向依赖）。
/// 返回本次解析结果便于日志 / 测试。
pub fn sync_settings(config: &ProxyConfigFile) -> Option<RateRecorderResolved> {
    let resolved = config
        .rate_recorder
        .as_ref()
        .map(resolve_section);
    if let Ok(mut slot) = settings_cell().write() {
        *slot = resolved.clone();
    }
    resolved
}

/// 读取当前生效快照；从未同步过（无段 / 未加载）返回 `None`（视为未启用）。
pub fn current_settings() -> Option<RateRecorderResolved> {
    settings_cell().read().ok().and_then(|slot| slot.clone())
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
