//! header_injection —— 渠道级「会话标记 → 请求头注入」配置段（fork 本地增强）。
//!
//! 与 `my_proxy_endpoint` / `rate_recorder` 同一 fork 补丁惯例：新增代码都在本目录，
//! 官方文件仅保留带横幅标记的注入点；恢复指南见本目录 `HEADER-INJECTION-PATCHES.md`。
//!
//! 本模块只做**纯逻辑**：`config.jsonc` 顶层 `header_injection` 段的解析结构、
//! 宽容反序列化、按渠道解析出运行时目标 Header。真正的请求体扫描与 Header 写入
//! 在 `token_proxy_runtime::proxy::header_injection`。

use std::collections::BTreeMap;

use axum::http::header::{HeaderName, ACCEPT_ENCODING, CONTENT_LENGTH, HOST};
use serde::{Deserialize, Serialize};

/// 当前支持的段版本；缺省按 1 处理，其他值整段忽略（向前兼容）。
pub const SUPPORTED_VERSION: u32 = 1;
/// canonical 会话规则 id。
pub const RULE_ID_SESSION: &str = "session-id";
/// 当前唯一支持的数据来源：Trae 钩子写入的会话标记。
pub const SOURCE_HOOK_SESSION_MARKER: &str = "hook_session_marker";
/// UI/CLI 缺省目标 Header（通用兼容性最好）。
pub const DEFAULT_TARGET_HEADER: &str = "X-Session-ID";

/// `config.jsonc` 顶层 `header_injection` 段。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeaderInjectionSection {
    /// 段版本；缺省按 [`SUPPORTED_VERSION`] 处理。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// 以渠道（upstream id）为键的配置。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub channels: BTreeMap<String, HeaderInjectionChannel>,
}

/// 单个渠道的头注入配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeaderInjectionChannel {
    /// 渠道级开关；缺省 false（所有渠道创建默认关闭）。
    pub enabled: bool,
    /// 注入规则列表（为扩展预留；当前只消费 canonical 会话规则）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<HeaderInjectionRule>,
}

/// 单条注入规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeaderInjectionRule {
    /// 规则标识；当前只认同 [`RULE_ID_SESSION`]。
    pub id: String,
    /// 规则开关；缺省 true（渠道开关已足以控制）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 数据来源；当前只支持 [`SOURCE_HOOK_SESSION_MARKER`]。
    pub source: String,
    /// 目标 Header 名。
    pub target_header: String,
}

impl Default for HeaderInjectionRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            source: String::new(),
            target_header: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// 归一化后的运行时配置：目标 Header 已校验、可直接写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderInjectionRuntime {
    pub target_header: HeaderName,
}

/// 容错反序列化：形态非法（字符串 / 数组 / 字段类型错误）时按「段不存在」处理，
/// 并记一条 warn。可选增强段 MUST NOT 阻塞配置加载。
pub fn de_lenient<'de, D>(deserializer: D) -> Result<Option<HeaderInjectionSection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if !value.is_object() {
        if !value.is_null() {
            tracing::warn!("header_injection section ignored: expected an object");
        }
        return Ok(None);
    }
    match serde_json::from_value::<HeaderInjectionSection>(value) {
        Ok(section) => Ok(Some(section)),
        Err(err) => {
            tracing::warn!(error = %err, "header_injection section ignored: invalid shape");
            Ok(None)
        }
    }
}

/// 解析某渠道的生效配置；未开启 / 规则不可用 / Header 名非法时返回 `None`。
pub fn resolve_channel(
    section: Option<&HeaderInjectionSection>,
    channel_id: &str,
) -> Option<HeaderInjectionRuntime> {
    let section = section?;
    let version = section.version.unwrap_or(SUPPORTED_VERSION);
    if version != SUPPORTED_VERSION {
        tracing::warn!(
            version,
            supported = SUPPORTED_VERSION,
            "header_injection section ignored: unsupported version"
        );
        return None;
    }

    let channel = section.channels.get(channel_id.trim())?;
    if !channel.enabled {
        return None;
    }

    for rule in &channel.rules {
        if !rule.enabled {
            continue;
        }
        if rule.id.trim() != RULE_ID_SESSION {
            continue;
        }
        if rule.source.trim() != SOURCE_HOOK_SESSION_MARKER {
            tracing::warn!(
                channel = channel_id,
                source = rule.source.as_str(),
                "header_injection rule ignored: unsupported source"
            );
            continue;
        }
        match normalize_target_header(&rule.target_header) {
            Ok(target_header) => return Some(HeaderInjectionRuntime { target_header }),
            Err(reason) => {
                tracing::warn!(
                    channel = channel_id,
                    header = rule.target_header.as_str(),
                    reason,
                    "header_injection rule ignored: invalid target header"
                );
                continue;
            }
        }
    }
    None
}

/// 校验并规范化目标 Header 名；保留名（Host / Content-Length / Accept-Encoding / hop-by-hop）被拒绝。
pub fn normalize_target_header(raw: &str) -> Result<HeaderName, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty target header".to_string());
    }
    let name = HeaderName::from_bytes(trimmed.as_bytes())
        .map_err(|_| "invalid header name".to_string())?;
    if is_reserved_header(&name) {
        return Err("reserved header name".to_string());
    }
    Ok(name)
}

fn is_reserved_header(name: &HeaderName) -> bool {
    *name == HOST
        || *name == CONTENT_LENGTH
        || *name == ACCEPT_ENCODING
        || matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-connection"
                | "transfer-encoding"
                | "upgrade"
                | "te"
                | "trailer"
                | "proxy-authenticate"
        )
}

impl HeaderInjectionSection {
    /// 启用（或更新）某渠道的 canonical 会话规则；CLI 与配置测试共用。
    ///
    /// `target_header` 缺省时沿用已有规则的目标 Header，再回落到 [`DEFAULT_TARGET_HEADER`]。
    pub fn enable_channel(
        &mut self,
        channel_id: &str,
        target_header: Option<&str>,
    ) -> Result<(), String> {
        let id = channel_id.trim();
        if id.is_empty() {
            return Err("channel id is required".to_string());
        }

        let existing = self
            .channels
            .get(id)
            .and_then(|channel| {
                channel
                    .rules
                    .iter()
                    .find(|rule| rule.id.trim() == RULE_ID_SESSION)
            })
            .map(|rule| rule.target_header.clone());

        let raw_header = target_header
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or(existing)
            .unwrap_or_else(|| DEFAULT_TARGET_HEADER.to_string());
        // 校验用运行时同一个入口；失败时返回错误且不落盘。
        normalize_target_header(&raw_header)?;
        self.version.get_or_insert(SUPPORTED_VERSION);

        let channel = self.channels.entry(id.to_string()).or_default();
        channel.enabled = true;
        if let Some(rule) = channel
            .rules
            .iter_mut()
            .find(|rule| rule.id.trim() == RULE_ID_SESSION)
        {
            rule.enabled = true;
            rule.source = SOURCE_HOOK_SESSION_MARKER.to_string();
            rule.target_header = raw_header;
        } else {
            channel.rules.push(HeaderInjectionRule {
                id: RULE_ID_SESSION.to_string(),
                enabled: true,
                source: SOURCE_HOOK_SESSION_MARKER.to_string(),
                target_header: raw_header,
            });
        }
        Ok(())
    }

    /// 关闭某渠道（保留记录与目标 Header）；返回是否命中了已有记录。
    pub fn disable_channel(&mut self, channel_id: &str) -> bool {
        let id = channel_id.trim();
        match self.channels.get_mut(id) {
            Some(channel) => {
                channel.enabled = false;
                true
            }
            None => false,
        }
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
