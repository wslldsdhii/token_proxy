//! `url_compose` 配置域：渠道级出站地址组合声明。
//!
//! 一条渠道可同时声明 openai / openai-response / anthropic / dashscope 四个接口家族的
//! `prefix`（特殊拼接）与 `suffix`（完整后缀，第一段为"版本段"），
//! 由 `compose.rs` 在出站时做纯拼接与版本段替换；`strip_version` 开启时匹配到的
//! 版本段在出站前整体去除。
//!
//! 严格协议透传（`strict_protocol_passthrough`）开启后，每个家族支持多条
//! `routes: [{prefix, suffix, strip_version}]`，由请求 URL 的后缀匹配决定协议；
//! 旧单条 `{prefix, suffix, strip_version}` 兼容为一条规则。

use serde::{Deserialize, Serialize};

/// 未配置家族（或 suffix 为空）时的回退版本段。
pub(crate) const DEFAULT_VERSION_SEGMENT: &str = "/v1";

/// 渠道级出站 URL 组合配置；键为接口家族（kebab-case：`openai-response`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UrlComposeConfig {
    /// 请求匹配协议透传：开启后协议只由 URL 规则匹配决定，不做协议转换。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict_protocol_passthrough: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<FamilyCompose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_response: Option<FamilyCompose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<FamilyCompose>,
    /// DashScope 原生协议家族（`/v1/services` 前缀透传的出站组合）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashscope: Option<FamilyCompose>,
}

/// 单接口家族：兼容旧单条 `{prefix, suffix, strip_version}` 与多规则 `routes`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyCompose {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strip_version: bool,
    /// 多规则；为空时使用上面的单条字段（旧格式）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<EndpointCompose>,
}

/// 单条 URL 规则/旧单条组合：prefix 与 suffix 均可留空。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCompose {
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// 去除版本段：开启后先按版本段匹配，再把匹配到的版本段整体去除。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strip_version: bool,
}

impl From<EndpointCompose> for FamilyCompose {
    fn from(value: EndpointCompose) -> Self {
        Self {
            prefix: value.prefix,
            suffix: value.suffix,
            strip_version: value.strip_version,
            routes: Vec::new(),
        }
    }
}

impl EndpointCompose {
    /// 规范化副本：去首尾空白、去结尾 `/`、非空时补开头 `/`。
    pub fn normalized(&self) -> Self {
        Self {
            prefix: normalize_segment(&self.prefix),
            suffix: normalize_segment(&self.suffix),
            strip_version: self.strip_version,
        }
    }

    /// 严格匹配键：suffix 去掉第一段（版本段）；不足两段返回 None。
    pub fn match_key(&self) -> Option<String> {
        match_key(&self.suffix)
    }
}

impl FamilyCompose {
    pub fn normalized(&self) -> Self {
        Self {
            prefix: normalize_segment(&self.prefix),
            suffix: normalize_segment(&self.suffix),
            strip_version: self.strip_version,
            routes: self.routes.iter().map(EndpointCompose::normalized).collect(),
        }
    }

    /// 统一规则视图：`routes` 非空用多规则，否则把单条字段视为一条规则。
    pub fn configured_routes(&self) -> Vec<EndpointCompose> {
        if !self.routes.is_empty() {
            return self.routes.clone();
        }
        if !self.prefix.is_empty() || !self.suffix.is_empty() || self.strip_version {
            vec![EndpointCompose {
                prefix: self.prefix.clone(),
                suffix: self.suffix.clone(),
                strip_version: self.strip_version,
            }]
        } else {
            Vec::new()
        }
    }

    pub fn primary_route(&self) -> Option<EndpointCompose> {
        self.configured_routes().into_iter().next()
    }

    /// 按客户端路径匹配本家族规则（最长前缀）。
    pub fn matching_route(&self, path: &str) -> Option<EndpointCompose> {
        let remainder = client_remainder(path)?;
        let mut best: Option<(EndpointCompose, usize)> = None;
        for route in self.configured_routes() {
            let Some(key) = route.match_key() else {
                continue;
            };
            if !key_matches(remainder, &key) {
                continue;
            }
            let key_len = key.len();
            if best
                .as_ref()
                .is_none_or(|(_, best_len)| key_len > *best_len)
            {
                best = Some((route, key_len));
            }
        }
        best.map(|(route, _)| route)
    }

    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty()
            && self.suffix.is_empty()
            && !self.strip_version
            && self.routes.is_empty()
    }
}

impl UrlComposeConfig {
    /// 深拷贝的规范化副本（配置加载期调用一次）。
    pub fn normalized(&self) -> Self {
        Self {
            strict_protocol_passthrough: self.strict_protocol_passthrough,
            openai: self.openai.as_ref().map(|value| value.normalized()),
            openai_response: self.openai_response.as_ref().map(|value| value.normalized()),
            anthropic: self.anthropic.as_ref().map(|value| value.normalized()),
            dashscope: self.dashscope.as_ref().map(|value| value.normalized()),
        }
    }

    /// 是否完全没有配置任何家族（用于旧 base_url 迁移提示）。
    pub fn is_empty(&self) -> bool {
        self.openai.as_ref().is_none_or(FamilyCompose::is_empty)
            && self
                .openai_response
                .as_ref()
                .is_none_or(FamilyCompose::is_empty)
            && self.anthropic.as_ref().is_none_or(FamilyCompose::is_empty)
            && self.dashscope.as_ref().is_none_or(FamilyCompose::is_empty)
    }

    /// 按 provider 字符串取接口家族。
    pub fn family(&self, provider: &str) -> Option<&FamilyCompose> {
        match provider {
            "openai" => self.openai.as_ref(),
            "openai-response" | "xai" => self.openai_response.as_ref(),
            "anthropic" => self.anthropic.as_ref(),
            "dashscope" => self.dashscope.as_ref(),
            _ => None,
        }
    }

    fn families(&self) -> [(&'static str, Option<&FamilyCompose>); 4] {
        [
            ("openai", self.openai.as_ref()),
            ("openai-response", self.openai_response.as_ref()),
            ("anthropic", self.anthropic.as_ref()),
            ("dashscope", self.dashscope.as_ref()),
        ]
    }

    /// 严格模式下跨家族匹配：返回命中家族名与规则（最长前缀）。
    pub fn match_strict_route(&self, path: &str) -> Option<(&'static str, EndpointCompose)> {
        let remainder = client_remainder(path)?;
        let mut best: Option<(&'static str, EndpointCompose, usize)> = None;
        for (family, value) in self.families() {
            let Some(value) = value else {
                continue;
            };
            for route in value.configured_routes() {
                let Some(key) = route.match_key() else {
                    continue;
                };
                if !key_matches(remainder, &key) {
                    continue;
                }
                let key_len = key.len();
                if best
                    .as_ref()
                    .is_none_or(|(_, _, best_len)| key_len > *best_len)
                {
                    best = Some((family, route, key_len));
                }
            }
        }
        best.map(|(family, route, _)| (family, route))
    }

    /// 严格模式配置校验：至少一条规则；每条规则必须有匹配键；匹配键不得重复。
    pub fn validate_strict_routes(&self) -> Result<(), String> {
        if !self.strict_protocol_passthrough {
            return Ok(());
        }
        let mut seen: std::collections::HashMap<String, &'static str> =
            std::collections::HashMap::new();
        let mut has_route = false;
        for (family, value) in self.families() {
            let Some(value) = value else {
                continue;
            };
            for route in value.configured_routes() {
                has_route = true;
                let Some(key) = route.match_key() else {
                    return Err(format!(
                        "url_compose.{family}: strict passthrough route suffix must contain a version segment and a sub-path"
                    ));
                };
                if let Some(previous) = seen.insert(key.clone(), family) {
                    return Err(format!(
                        "url_compose: duplicate strict route match key {key} in {previous} and {family}"
                    ));
                }
            }
        }
        if !has_route {
            return Err(
                "url_compose.strict_protocol_passthrough requires at least one route".to_string(),
            );
        }
        Ok(())
    }

    /// 严格模式家族必须与渠道 providers 对齐（如 openai 家族需要 providers 含 openai）。
    pub fn validate_strict_family_providers(&self, providers: &[String]) -> Result<(), String> {
        if !self.strict_protocol_passthrough {
            return Ok(());
        }
        for (family, value) in self.families() {
            let Some(value) = value else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            if !providers.iter().any(|provider| provider == family) {
                return Err(format!(
                    "url_compose.{family}: upstream providers must include \"{family}\" for strict protocol passthrough"
                ));
            }
        }
        Ok(())
    }
}

/// 段规范化：`"anthropic/"` → `"/anthropic"`、`"openai"` → `"/openai"`、`""` → `""`。
pub(crate) fn normalize_segment(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// suffix 去掉第一段（版本段）后的匹配键；不足两段返回 None。
pub fn match_key(suffix: &str) -> Option<String> {
    let normalized = normalize_segment(suffix);
    let stripped = normalized.strip_prefix('/')?;
    let (first, rest) = stripped.split_once('/')?;
    if first.is_empty() || rest.is_empty() {
        return None;
    }
    Some(format!("/{rest}").trim_end_matches('/').to_string())
}

/// 客户端路径去掉第一段（版本段）后的 remainder；保留开头 `/`。
fn client_remainder(path: &str) -> Option<&str> {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    let stripped = path.strip_prefix('/')?;
    let (first, _) = stripped.split_once('/')?;
    if first.is_empty() {
        return None;
    }
    let remainder = &stripped[first.len()..];
    (!remainder.is_empty()).then_some(remainder)
}

/// remainder 与匹配键相等，或 remainder 以 `键 + /` 开头（子路径）。
fn key_matches(remainder: &str, key: &str) -> bool {
    remainder == key
        || remainder
            .strip_prefix(key)
            .is_some_and(|tail| tail.starts_with('/'))
}