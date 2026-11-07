use super::needs_legacy_version_warn;
use super::{EndpointCompose, FamilyCompose};
use super::UrlComposeConfig;
use crate::types::{ProxyConfigFile, UpstreamConfig};

#[test]
fn url_compose_serializes_kebab_case_keys() {
    let upstream = UpstreamConfig {
        id: "u1".to_string(),
        providers: vec!["openai-response".to_string()],
        base_url: "https://x.com".to_string(),
        credential: crate::types::UpstreamCredential::api_keys(["k"]),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: Default::default(),
        convert_from_map: Default::default(),
        overrides: None,
        url_compose: Some(UrlComposeConfig {
            openai_response: Some(FamilyCompose {
                prefix: String::new(),
                suffix: "/v3/responses".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
    };
    let value = serde_json::to_value(&upstream).expect("serialize");
    assert_eq!(value["url_compose"]["openai-response"]["suffix"], "/v3/responses");
    assert!(value["url_compose"].get("openai").is_none());
    assert!(value["url_compose"].get("anthropic").is_none());
}

// ══════════ MY-STRIP-VERSION PATCH 4 (test) START ══════════
#[test]
fn url_compose_strip_version_serializes_only_when_true() {
    let upstream = UpstreamConfig {
        id: "u1".to_string(),
        providers: vec!["openai".to_string()],
        base_url: "https://x.com".to_string(),
        credential: crate::types::UpstreamCredential::api_keys(["k"]),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: Default::default(),
        convert_from_map: Default::default(),
        overrides: None,
        url_compose: Some(UrlComposeConfig {
            openai: Some(FamilyCompose {
                prefix: String::new(),
                suffix: "/v1/chat/completions".to_string(),
                strip_version: true,
                routes: Vec::new(),
            }),
            ..Default::default()
        }),
    };
    let value = serde_json::to_value(&upstream).expect("serialize");
    assert_eq!(value["url_compose"]["openai"]["strip_version"], true);

    // strip_version 为 false 时键不落盘（与缺省等价，配置保持精简）。
    let disabled = UpstreamConfig {
        url_compose: Some(UrlComposeConfig {
            openai: Some(FamilyCompose {
                prefix: String::new(),
                suffix: "/v1/chat/completions".to_string(),
                strip_version: false,
                routes: Vec::new(),
            }),
            ..Default::default()
        }),
        ..upstream
    };
    let value = serde_json::to_value(&disabled).expect("serialize");
    assert!(value["url_compose"]["openai"].get("strip_version").is_none());
}

#[test]
fn url_compose_without_strip_version_field_defaults_false() {
    // 本变更之前的配置文件无 strip_version 字段：加载成功且等价 false。
    let config: ProxyConfigFile = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1",
        "port": 8118,
        "upstreams": [{
            "id": "u1",
            "providers": ["openai"],
            "base_url": "https://x.com",
            "credential": { "type": "api_keys", "api_keys": ["k"] },
            "url_compose": { "openai": { "prefix": "", "suffix": "/v1/chat/completions" } }
        }]
    }))
    .expect("config without strip_version must load");
    let compose = config.upstreams[0]
        .url_compose
        .as_ref()
        .unwrap()
        .openai
        .as_ref()
        .unwrap();
    assert!(!compose.strip_version);
}
// ══════════ MY-STRIP-VERSION PATCH 4 (test) END ══════════

#[test]
fn legacy_my_conf_fields_are_silently_ignored() {
    let config: ProxyConfigFile = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1",
        "port": 8118,
        "upstreams": [{
            "id": "u1",
            "providers": ["openai"],
            "base_url": "https://x.com",
            "my_conf_openai": { "openai_compatible_v1_fix": false },
            "my_conf_anthropic": { "anthropic_messages_url_fix": false }
        }]
    }))
    .expect("legacy config with my_conf fields must load");
    assert_eq!(config.upstreams.len(), 1);
    assert!(config.upstreams[0].url_compose.is_none());
}

#[test]
fn url_compose_missing_falls_back_to_identity() {
    let config: ProxyConfigFile = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1",
        "port": 8118,
        "upstreams": [{
            "id": "u1",
            "providers": ["anthropic"],
            "base_url": "https://x.com",
            "credential": { "type": "api_keys", "api_keys": ["k"] }
        }]
    }))
    .expect("config");
    assert!(config.upstreams[0].url_compose.is_none());
}

#[test]
fn warn_needed_when_fully_unconfigured_and_versioned_base() {
    assert!(needs_legacy_version_warn("https://x.com/v1", None));
    assert!(needs_legacy_version_warn(
        "https://x.com/v1",
        Some(&UrlComposeConfig::default())
    ));
    assert!(needs_legacy_version_warn("https://x.com/v3/", None));
}

#[test]
fn warn_skipped_when_any_family_configured() {
    let compose = UrlComposeConfig {
        openai: Some(FamilyCompose {
            prefix: String::new(),
            suffix: "/v3/chat/completions".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!needs_legacy_version_warn("https://x.com/v1", Some(&compose)));
}

#[test]
fn warn_skipped_for_non_version_base() {
    assert!(!needs_legacy_version_warn("https://x.com/api/plan", None));
    assert!(!needs_legacy_version_warn("https://x.com", None));
    assert!(!needs_legacy_version_warn("https://x.com/v", None));
}

// ══════════ STRICT-PROTOCOL-PASSTHROUGH TESTS START ══════════
fn route(prefix: &str, suffix: &str) -> EndpointCompose {
    EndpointCompose {
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
        strip_version: false,
    }
}

fn strict_config() -> UrlComposeConfig {
    UrlComposeConfig {
        strict_protocol_passthrough: true,
        openai: Some(FamilyCompose {
            routes: vec![
                route("/compatible-mode", "/v1/chat/completions"),
                route("/compatible-mode", "/v1/embedding"),
                route("/compatible-mode", "/v1/rerank"),
            ],
            ..Default::default()
        }),
        openai_response: Some(FamilyCompose {
            routes: vec![route("/compatible-mode", "/v1/responses")],
            ..Default::default()
        }),
        anthropic: Some(FamilyCompose {
            routes: vec![route("/anthropic", "/v1/messages")],
            ..Default::default()
        }),
        dashscope: Some(FamilyCompose {
            routes: vec![route(
                "/api",
                "/v3/services/aigc/text-generation/generation",
            )],
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn strict_route_matches_chat_and_embedding_under_openai() {
    let config = strict_config();
    let (family, route) = config
        .match_strict_route("/v1/chat/completions")
        .expect("chat route");
    assert_eq!(family, "openai");
    assert_eq!(route.suffix, "/v1/chat/completions");

    let (family, route) = config
        .match_strict_route("/v1/embedding")
        .expect("embedding route");
    assert_eq!(family, "openai");
    assert_eq!(route.suffix, "/v1/embedding");
}

#[test]
fn strict_route_does_not_route_embedding_to_responses() {
    let config = strict_config();
    let (family, _) = config
        .match_strict_route("/v1/embedding")
        .expect("embedding route");
    assert_eq!(family, "openai");
    assert!(config
        .family("openai-response")
        .expect("responses family")
        .matching_route("/v1/embedding")
        .is_none());
}

#[test]
fn strict_route_longest_prefix_wins() {
    let config = UrlComposeConfig {
        strict_protocol_passthrough: true,
        openai_response: Some(FamilyCompose {
            routes: vec![
                route("/openai", "/v1/responses"),
                route("/openai", "/v1/responses/input_tokens"),
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    let (_, route) = config
        .match_strict_route("/v1/responses/input_tokens")
        .expect("longest route");
    assert_eq!(route.suffix, "/v1/responses/input_tokens");
}

#[test]
fn strict_route_supports_v3_upstream_version() {
    let config = UrlComposeConfig {
        strict_protocol_passthrough: true,
        dashscope: Some(FamilyCompose {
            routes: vec![route(
                "/api",
                "/v3/services/aigc/text-generation/generation",
            )],
            ..Default::default()
        }),
        ..Default::default()
    };
    let (family, route) = config
        .match_strict_route("/v1/services/aigc/text-generation/generation")
        .expect("dashscope route");
    assert_eq!(family, "dashscope");
    assert_eq!(route.suffix, "/v3/services/aigc/text-generation/generation");
}

#[test]
fn strict_route_returns_none_for_unknown_path() {
    let config = strict_config();
    assert!(config.match_strict_route("/v1/unknown-endpoint").is_none());
}

#[test]
fn strict_route_accepts_subpath() {
    let config = strict_config();
    let (family, route) = config
        .match_strict_route("/v1/messages/count_tokens")
        .expect("subpath route");
    assert_eq!(family, "anthropic");
    assert_eq!(route.suffix, "/v1/messages");
}

#[test]
fn strict_route_ignores_query() {
    let config = strict_config();
    let (family, _) = config
        .match_strict_route("/v1/chat/completions?foo=bar")
        .expect("query route");
    assert_eq!(family, "openai");
}

#[test]
fn strict_validation_rejects_duplicate_match_keys() {
    let config = UrlComposeConfig {
        strict_protocol_passthrough: true,
        openai: Some(FamilyCompose {
            routes: vec![route("/a", "/v1/chat/completions")],
            ..Default::default()
        }),
        openai_response: Some(FamilyCompose {
            routes: vec![route("/b", "/v1/chat/completions")],
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = config.validate_strict_routes().expect_err("duplicate");
    assert!(error.contains("duplicate strict route match key"));
}

#[test]
fn strict_validation_requires_at_least_one_route() {
    let config = UrlComposeConfig {
        strict_protocol_passthrough: true,
        ..Default::default()
    };
    let error = config.validate_strict_routes().expect_err("empty");
    assert!(error.contains("requires at least one route"));
}

#[test]
fn strict_validation_accepts_valid_config() {
    strict_config().validate_strict_routes().expect("valid");
}

#[test]
fn old_single_family_is_one_route() {
    let family = FamilyCompose {
        prefix: "/openai".to_string(),
        suffix: "/v1/responses".to_string(),
        ..Default::default()
    };
    let routes = family.configured_routes();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].prefix, "/openai");
    assert_eq!(routes[0].suffix, "/v1/responses");
}

#[test]
fn strict_flag_serializes_only_when_true() {
    let value = serde_json::to_value(strict_config()).expect("serialize");
    assert_eq!(value["strict-protocol-passthrough"], true);
    let disabled = serde_json::to_value(UrlComposeConfig::default()).expect("serialize");
    assert!(disabled.get("strict-protocol-passthrough").is_none());
}
// ══════════ STRICT-PROTOCOL-PASSTHROUGH TESTS END ══════════
