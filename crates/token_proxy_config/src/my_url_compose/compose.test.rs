use super::{compose_from_parts, version_segment};
use crate::my_url_compose::config::normalize_segment;
use crate::my_url_compose::EndpointCompose;

fn compose(base: &str, prefix: &str, suffix: &str, path: &str) -> String {
    let endpoint = EndpointCompose {
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
        ..Default::default()
    };
    compose_from_parts(base, Some(&endpoint), path)
}

// ══════════ MY-STRIP-VERSION PATCH 3 (test) START ══════════
fn compose_strip(base: &str, prefix: &str, suffix: &str, path: &str) -> String {
    let endpoint = EndpointCompose {
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
        strip_version: true,
    };
    compose_from_parts(base, Some(&endpoint), path)
}
// ══════════ MY-STRIP-VERSION PATCH 3 (test) END ══════════

fn unconfigured(base: &str, path: &str) -> String {
    compose_from_parts(base, None, path)
}

// ── 恒等回退（未配置家族） ────────────────────────────────────────────

#[test]
fn unconfigured_family_is_identity_join() {
    assert_eq!(
        unconfigured("https://api.example.com", "/v1/chat/completions"),
        "https://api.example.com/v1/chat/completions"
    );
    assert_eq!(
        unconfigured("https://api.example.com", "/v1/models/gpt-5"),
        "https://api.example.com/v1/models/gpt-5"
    );
}

#[test]
fn configured_empty_suffix_falls_back_to_identity() {
    assert_eq!(
        compose("https://x.com", "", "", "/v1/chat/completions"),
        "https://x.com/v1/chat/completions"
    );
}

// ── 版本段替换 ───────────────────────────────────────────────────────

#[test]
fn v3_suffix_replaces_first_segment_only() {
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/chat/completions"),
        "https://ark.example.com/api/plan/v3/chat/completions"
    );
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/models"),
        "https://ark.example.com/api/plan/v3/models"
    );
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/embeddings"),
        "https://ark.example.com/api/plan/v3/embeddings"
    );
}

#[test]
fn arbitrary_version_segment_like_abc() {
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/chat/completions"),
        "https://x.com/abc/chat/completions"
    );
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/abcdefg/xxx"),
        "https://x.com/abc/abcdefg/xxx"
    );
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/models"),
        "https://x.com/abc/models"
    );
}

#[test]
fn anthropic_prefix_and_subpaths_follow() {
    assert_eq!(
        compose("https://x.com", "/anthropic", "/v1/messages", "/v1/messages"),
        "https://x.com/anthropic/v1/messages"
    );
    assert_eq!(
        compose("https://x.com", "/anthropic", "/v1/messages", "/v1/messages/count_tokens"),
        "https://x.com/anthropic/v1/messages/count_tokens"
    );
}

#[test]
fn plain_path_exactly_v1_is_replaced() {
    assert_eq!(
        compose("https://x.com", "", "/v3/models", "/v1"),
        "https://x.com/v3"
    );
}

#[test]
fn v10_is_not_matched_as_v1() {
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/v10/models"),
        "https://x.com/v10/models"
    );
}

#[test]
fn non_v1_first_segments_pass_through() {
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/v1beta/openai/models"),
        "https://x.com/v1beta/openai/models"
    );
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/alpha/search"),
        "https://x.com/alpha/search"
    );
}

// ── query / base 边界 ────────────────────────────────────────────────

#[test]
fn query_is_preserved_verbatim() {
    assert_eq!(
        compose("https://x.com", "", "/v3/models", "/v1/models?limit=100&b=2"),
        "https://x.com/v3/models?limit=100&b=2"
    );
}

#[test]
fn base_trailing_slash_is_trimmed() {
    assert_eq!(
        compose("https://x.com/", "", "/v1/chat/completions", "/v1/chat/completions"),
        "https://x.com/v1/chat/completions"
    );
}

#[test]
fn empty_base_joins_prefix_and_path() {
    assert_eq!(
        compose("", "/anthropic", "/v1/messages", "/v1/messages"),
        "/anthropic/v1/messages"
    );
}

// ── 版本段解析与规范化 ───────────────────────────────────────────────

#[test]
fn version_segment_defaults_and_extracts() {
    assert_eq!(version_segment(""), "/v1");
    assert_eq!(version_segment("   "), "/v1");
    assert_eq!(version_segment("/v3/chat/completions"), "/v3");
    assert_eq!(version_segment("/abc"), "/abc");
    assert_eq!(version_segment("v3"), "/v3");
    assert_eq!(version_segment("/v3/"), "/v3");
}

#[test]
fn segment_normalization_rules() {
    assert_eq!(normalize_segment("anthropic/"), "/anthropic");
    assert_eq!(normalize_segment("openai"), "/openai");
    assert_eq!(normalize_segment(" /v1 "), "/v1");
    assert_eq!(normalize_segment(""), "");
    assert_eq!(normalize_segment("/"), "");
}

#[test]
fn config_normalized_deep_copies() {
    use crate::my_url_compose::UrlComposeConfig;
    let mut config = UrlComposeConfig::default();
    config.anthropic = Some(EndpointCompose {
        prefix: "anthropic/".to_string(),
        suffix: " /v1/messages ".to_string(),
        ..Default::default()
    });
    let normalized = config.normalized();
    let anthropic = normalized.anthropic.as_ref().unwrap();
    assert_eq!(anthropic.prefix, "/anthropic");
    assert_eq!(anthropic.suffix, "/v1/messages");
}

// ── bigmodel 特例迁移等价性 ──────────────────────────────────────────

#[test]
fn bigmodel_paas_base_via_compose() {
    // 官方特例（已删除）：base 含 /api/coding/paas/ 时把 /v1/chat/completions
    // 改写为 /chat/completions。等价写法：suffix = /chat/completions。
    assert_eq!(
        compose(
            "https://open.bigmodel.cn/api/coding/paas",
            "",
            "/v1/chat/completions",
            "/v1/chat/completions"
        ),
        "https://open.bigmodel.cn/api/coding/paas/v1/chat/completions"
    );
}

// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (test) START ══════════
// ── dashscope 家族（/v1/services 前缀透传的出站组合） ─────────────────

#[test]
fn dashscope_native_pure_concat() {
    // 入站 /v1/services/*，suffix 版本段恰为 /v1：base + /api + 原样子路径。
    assert_eq!(
        compose(
            "https://ws.cn-beijing.maas.aliyuncs.com",
            "/api",
            "/v1/services/aigc/text-generation/generation",
            "/v1/services/aigc/multimodal-generation/generation"
        ),
        "https://ws.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
    );
}

#[test]
fn dashscope_embedding_rerank_share_same_config() {
    let base = "https://ws.cn-beijing.maas.aliyuncs.com";
    assert_eq!(
        compose(
            base,
            "/api",
            "/v1/services/aigc/text-generation/generation",
            "/v1/services/embeddings/text-embedding/text-embedding"
        ),
        "https://ws.cn-beijing.maas.aliyuncs.com/api/v1/services/embeddings/text-embedding/text-embedding"
    );
    assert_eq!(
        compose(
            base,
            "/api",
            "/v1/services/aigc/text-generation/generation",
            "/v1/services/rerank/text-rerank/text-rerank"
        ),
        "https://ws.cn-beijing.maas.aliyuncs.com/api/v1/services/rerank/text-rerank/text-rerank"
    );
}

#[test]
fn dashscope_version_segment_replacement() {
    // 非默认版本段同样生效：入站首段 /v1 替换为 suffix 第一段。
    assert_eq!(
        compose(
            "https://x.com",
            "/api",
            "/v3/gen",
            "/v1/services/aigc/x/generation"
        ),
        "https://x.com/api/v3/services/aigc/x/generation"
    );
}

#[test]
fn dashscope_empty_family_falls_back_to_identity() {
    assert_eq!(
        compose(
            "https://ws.cn-beijing.maas.aliyuncs.com",
            "",
            "",
            "/v1/services/aigc/x/generation"
        ),
        "https://ws.cn-beijing.maas.aliyuncs.com/v1/services/aigc/x/generation"
    );
}
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (test) END ══════════

// ══════════ MY-STRIP-VERSION PATCH 3 (test) START ══════════
// ── 去除版本段（strip_version） ──────────────────────────────────────

#[test]
fn strip_removes_version_segment_after_match() {
    // 基础语义：suffix 仍填 /v1/chat/completions，出站不再携带 /v1。
    assert_eq!(
        compose_strip("https://x.com", "", "/v1/chat/completions", "/v1/chat/xxxx"),
        "https://x.com/chat/xxxx"
    );
    assert_eq!(
        compose_strip("https://x.com", "", "/v1/chat/completions", "/v1/models"),
        "https://x.com/models"
    );
}

#[test]
fn strip_keeps_prefix_for_prefixless_vendor() {
    // 无版本号渠道商 + 特殊拼接：anthropic 直连 /anthropic/messages。
    assert_eq!(
        compose_strip("https://x.com", "/anthropic", "/v1/messages", "/v1/messages"),
        "https://x.com/anthropic/messages"
    );
    assert_eq!(
        compose_strip("https://x.com", "/anthropic", "/v1/messages", "/v1/messages/count_tokens"),
        "https://x.com/anthropic/messages/count_tokens"
    );
}

#[test]
fn strip_removes_arbitrary_version_segment_like_v3() {
    // 版本段区域填 /v3 等任意值：替换后同样被去除，无额外语义。
    assert_eq!(
        compose_strip("https://x.com", "", "/v3/chat/completions", "/v1/chat/xxxx"),
        "https://x.com/chat/xxxx"
    );
    assert_eq!(
        compose_strip("https://x.com", "", "/abc/chat/completions", "/v1/abcdefg/xxx"),
        "https://x.com/abcdefg/xxx"
    );
}

#[test]
fn strip_leaves_non_v1_first_segments_untouched() {
    // 非 /v1 首段：不替换也不去除，与未开启时逐字节一致。
    assert_eq!(
        compose_strip("https://x.com", "", "/v3/chat/completions", "/v1beta/openai/models"),
        "https://x.com/v1beta/openai/models"
    );
    assert_eq!(
        compose_strip("https://x.com", "", "/v3/chat/completions", "/alpha/search"),
        "https://x.com/alpha/search"
    );
    assert_eq!(
        compose_strip("https://x.com", "", "/v3/chat/completions", "/v10/models"),
        "https://x.com/v10/models"
    );
}

#[test]
fn strip_with_empty_suffix_uses_fallback_version_segment() {
    // 空 suffix：回退版本段 /v1 参与匹配与去除。
    assert_eq!(
        compose_strip("https://x.com", "", "", "/v1/models"),
        "https://x.com/models"
    );
}

#[test]
fn strip_path_exactly_v1_yields_empty_path() {
    // 客户端路径恰为 /v1：版本段去除后路径为空，出站 = base + prefix。
    assert_eq!(
        compose_strip("https://x.com", "", "/v1/chat/completions", "/v1"),
        "https://x.com"
    );
    assert_eq!(
        compose_strip("https://x.com", "/anthropic", "/v1/messages", "/v1"),
        "https://x.com/anthropic"
    );
}

#[test]
fn strip_preserves_query_verbatim() {
    assert_eq!(
        compose_strip("https://x.com", "", "/v1/chat/completions", "/v1/models?limit=100&b=2"),
        "https://x.com/models?limit=100&b=2"
    );
}

#[test]
fn strip_disabled_keeps_replacement_byte_identical() {
    // strip_version 缺省 false：与既有替换行为逐字节一致。
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/v1/chat/xxxx"),
        "https://x.com/v3/chat/xxxx"
    );
}

#[test]
fn strip_applies_to_dashscope_family() {
    assert_eq!(
        compose_strip(
            "https://ws.cn-beijing.maas.aliyuncs.com",
            "/api",
            "/v1/services/aigc/text-generation/generation",
            "/v1/services/aigc/x/generation"
        ),
        "https://ws.cn-beijing.maas.aliyuncs.com/api/services/aigc/x/generation"
    );
}

#[test]
fn normalized_preserves_strip_version() {
    let mut config = crate::my_url_compose::UrlComposeConfig::default();
    config.openai = Some(EndpointCompose {
        prefix: String::new(),
        suffix: "/v1/chat/completions".to_string(),
        strip_version: true,
    });
    let normalized = config.normalized();
    assert!(normalized.openai.as_ref().unwrap().strip_version);
}
// ══════════ MY-STRIP-VERSION PATCH 3 (test) END ══════════
