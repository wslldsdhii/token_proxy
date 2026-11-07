//! 严格协议透传：开启后协议只由请求 URL 的 `url_compose` 规则匹配决定，
//! 不做请求/响应协议转换，也不跨协议回退。

use super::super::{
    config::{ProxyConfig, UpstreamRuntime},
    openai_compat::{FormatTransform, PROVIDER_CHAT, PROVIDER_RESPONSES},
    RequestMeta,
};
use super::{DispatchPlan, PROVIDER_ANTHROPIC, PROVIDER_DASHSCOPE};

const STRICT_EXEMPT_PREFIXES: &[&str] = &["/v1/models", "/v1beta/models", "/v1beta/openai/models"];

/// 在拿到 `RequestMeta` 后覆盖 plan：目标渠道开启严格透传时，按 URL 规则锁定协议。
pub(super) fn apply_strict_plan(
    config: &ProxyConfig,
    path: &str,
    meta: &RequestMeta,
    plan: DispatchPlan,
) -> Result<DispatchPlan, String> {
    if is_exempt_path(path) {
        return Ok(plan);
    }
    let Some(upstream) = find_target_upstream(config, meta.original_model.as_deref()) else {
        return Ok(plan);
    };
    if !upstream.url_compose.strict_protocol_passthrough {
        return Ok(plan);
    }
    let Some((family, _route)) = upstream.url_compose.match_strict_route(path) else {
        return Err(format!(
            "strict protocol passthrough: no url_compose route matches {path} for upstream {}",
            upstream.id
        ));
    };
    let Some(provider) = provider_for_family(family) else {
        return Err(format!(
            "strict protocol passthrough: unsupported url_compose family {family}"
        ));
    };
    Ok(DispatchPlan {
        provider,
        outbound_path: None,
        request_transform: FormatTransform::None,
        response_transform: FormatTransform::None,
        strict_passthrough: true,
    })
}

/// 从模型前缀 `渠道/模型` 找到目标渠道；找不到则严格模式不生效。
fn find_target_upstream<'a>(
    config: &'a ProxyConfig,
    original_model: Option<&str>,
) -> Option<&'a UpstreamRuntime> {
    let original_model = original_model?.trim();
    let (prefix, rest) = original_model.split_once('/')?;
    if prefix.trim().is_empty() || rest.trim().is_empty() {
        return None;
    }
    for provider in [
        "openai",
        "openai-response",
        "anthropic",
        "dashscope",
        "xai",
        "codex",
        "kiro",
        "gemini",
    ] {
        let Some(upstreams) = config.provider_upstreams(provider) else {
            continue;
        };
        if let Some(upstream) = upstreams
            .groups
            .iter()
            .flat_map(|group| group.items.iter())
            .find(|upstream| upstream.id == prefix)
        {
            return Some(upstream);
        }
    }
    None
}

fn provider_for_family(family: &str) -> Option<&'static str> {
    match family {
        "openai" => Some(PROVIDER_CHAT),
        "openai-response" => Some(PROVIDER_RESPONSES),
        "anthropic" => Some(PROVIDER_ANTHROPIC),
        "dashscope" => Some(PROVIDER_DASHSCOPE),
        _ => None,
    }
}

/// 模型目录/资源端点不参与严格 URL 匹配。
fn is_exempt_path(path: &str) -> bool {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    STRICT_EXEMPT_PREFIXES.iter().any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exempt_paths_cover_models() {
        assert!(is_exempt_path("/v1/models"));
        assert!(is_exempt_path("/v1/models/gpt-5"));
        assert!(is_exempt_path("/v1beta/models/gemini-1.5"));
        assert!(is_exempt_path("/v1beta/openai/models"));
        assert!(!is_exempt_path("/v1/chat/completions"));
        assert!(!is_exempt_path("/v1/responses/input_tokens"));
    }

    #[test]
    fn family_maps_to_provider() {
        assert_eq!(provider_for_family("openai"), Some(PROVIDER_CHAT));
        assert_eq!(
            provider_for_family("openai-response"),
            Some(PROVIDER_RESPONSES)
        );
        assert_eq!(
            provider_for_family("anthropic"),
            Some(PROVIDER_ANTHROPIC)
        );
        assert_eq!(
            provider_for_family("dashscope"),
            Some(PROVIDER_DASHSCOPE)
        );
        assert_eq!(provider_for_family("unknown"), None);
    }
}