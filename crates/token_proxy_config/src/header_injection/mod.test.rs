use super::*;

fn rule(id: &str, enabled: bool, source: &str, header: &str) -> HeaderInjectionRule {
    HeaderInjectionRule {
        id: id.to_string(),
        enabled,
        source: source.to_string(),
        target_header: header.to_string(),
    }
}

fn channel(enabled: bool, rules: Vec<HeaderInjectionRule>) -> HeaderInjectionChannel {
    HeaderInjectionChannel { enabled, rules }
}

fn section_with(enabled: bool, header: &str) -> HeaderInjectionSection {
    let mut channels = BTreeMap::new();
    channels.insert(
        "trae".to_string(),
        channel(
            enabled,
            vec![rule(
                RULE_ID_SESSION,
                true,
                SOURCE_HOOK_SESSION_MARKER,
                header,
            )],
        ),
    );
    HeaderInjectionSection {
        version: Some(SUPPORTED_VERSION),
        channels,
    }
}

#[test]
fn missing_section_and_channel_resolve_to_none() {
    assert!(resolve_channel(None, "trae").is_none());
    let section = section_with(true, DEFAULT_TARGET_HEADER);
    assert!(resolve_channel(Some(&section), "other").is_none());
}

#[test]
fn disabled_channel_or_rule_resolves_to_none() {
    let section = section_with(false, DEFAULT_TARGET_HEADER);
    assert!(resolve_channel(Some(&section), "trae").is_none());

    let mut section = section_with(true, DEFAULT_TARGET_HEADER);
    section.channels.get_mut("trae").unwrap().rules[0].enabled = false;
    assert!(resolve_channel(Some(&section), "trae").is_none());
}

#[test]
fn enabled_channel_resolves_default_header() {
    let section = section_with(true, DEFAULT_TARGET_HEADER);
    let runtime = resolve_channel(Some(&section), " trae ").expect("resolved");
    assert_eq!(
        runtime.target_header,
        HeaderName::from_static("x-session-id")
    );
}

#[test]
fn enabled_channel_resolves_opencode_header() {
    let section = section_with(true, "X-Opencode-Session");
    let runtime = resolve_channel(Some(&section), "trae").expect("resolved");
    assert_eq!(
        runtime.target_header,
        HeaderName::from_static("x-opencode-session")
    );
}

#[test]
fn unsupported_source_is_ignored() {
    let section = section_with(true, DEFAULT_TARGET_HEADER);
    let mut section = section.clone();
    section.channels.get_mut("trae").unwrap().rules[0].source = "other_source".to_string();
    assert!(resolve_channel(Some(&section), "trae").is_none());

    // 未知 source 不影响同渠道后续合法规则。
    let mut section = section;
    section.channels.get_mut("trae").unwrap().rules.push(rule(
        RULE_ID_SESSION,
        true,
        SOURCE_HOOK_SESSION_MARKER,
        "X-Opencode-Session",
    ));
    let runtime = resolve_channel(Some(&section), "trae").expect("resolved");
    assert_eq!(
        runtime.target_header,
        HeaderName::from_static("x-opencode-session")
    );
}

#[test]
fn invalid_or_reserved_header_is_ignored() {
    for header in [
        "",
        "bad header",
        "bad:header",
        "Host",
        "Content-Length",
        "Accept-Encoding",
        "Connection",
    ] {
        let section = section_with(true, header);
        assert!(
            resolve_channel(Some(&section), "trae").is_none(),
            "{header} should be rejected"
        );
    }
}

#[test]
fn unsupported_version_ignores_whole_section() {
    let mut section = section_with(true, DEFAULT_TARGET_HEADER);
    section.version = Some(SUPPORTED_VERSION + 1);
    assert!(resolve_channel(Some(&section), "trae").is_none());
}

#[test]
fn missing_version_is_treated_as_supported() {
    let mut section = section_with(true, DEFAULT_TARGET_HEADER);
    section.version = None;
    assert!(resolve_channel(Some(&section), "trae").is_some());
}

#[derive(serde::Deserialize)]
struct Wrapper {
    #[serde(default, deserialize_with = "de_lenient")]
    header_injection: Option<HeaderInjectionSection>,
}

#[test]
fn lenient_deserializer_ignores_non_object_shapes() {
    let parsed: Wrapper =
        serde_json::from_str(r#"{"header_injection":[]}"#).expect("array tolerated");
    assert!(parsed.header_injection.is_none());

    let parsed: Wrapper =
        serde_json::from_str(r#"{"header_injection":"oops"}"#).expect("string tolerated");
    assert!(parsed.header_injection.is_none());
}

#[test]
fn lenient_deserializer_ignores_broken_field_types() {
    let parsed: Wrapper = serde_json::from_str(
        r#"{"header_injection":{"version":1,"channels":{"trae":{"enabled":"yes"}}}}"#,
    )
    .expect("broken section tolerated");
    assert!(parsed.header_injection.is_none());
}

#[test]
fn section_round_trips_through_json() {
    let section = section_with(true, "X-Opencode-Session");
    let json = serde_json::to_string(&section).expect("serialize");
    let parsed: HeaderInjectionSection = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, section);
}

#[test]
fn rule_enabled_defaults_to_true() {
    let parsed: HeaderInjectionRule = serde_json::from_str(
        r#"{"id":"session-id","source":"hook_session_marker","target_header":"X-Session-ID"}"#,
    )
    .expect("rule");
    assert!(parsed.enabled);
}

#[test]
fn enable_channel_creates_canonical_rule_with_default_header() {
    let mut section = HeaderInjectionSection::default();
    section.enable_channel("trae", None).expect("enable");
    assert_eq!(section.version, Some(SUPPORTED_VERSION));
    let channel = section.channels.get("trae").expect("channel");
    assert!(channel.enabled);
    assert_eq!(channel.rules.len(), 1);
    assert_eq!(channel.rules[0].id, RULE_ID_SESSION);
    assert!(channel.rules[0].enabled);
    assert_eq!(channel.rules[0].source, SOURCE_HOOK_SESSION_MARKER);
    assert_eq!(channel.rules[0].target_header, DEFAULT_TARGET_HEADER);
}

#[test]
fn enable_channel_preserves_existing_header_when_omitted() {
    let mut section = section_with(false, "X-Opencode-Session");
    section.enable_channel("trae", None).expect("enable");
    let channel = section.channels.get("trae").expect("channel");
    assert!(channel.enabled);
    assert_eq!(channel.rules[0].target_header, "X-Opencode-Session");
}

#[test]
fn enable_channel_updates_header_and_preserves_extra_rules() {
    let mut section = section_with(false, DEFAULT_TARGET_HEADER);
    section.channels.get_mut("trae").unwrap().rules.push(rule(
        "future-rule",
        true,
        "future_source",
        "X-Future",
    ));
    section
        .enable_channel("trae", Some("X-Opencode-Session"))
        .expect("enable");
    let rules = &section.channels.get("trae").unwrap().rules;
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].target_header, "X-Opencode-Session");
    assert_eq!(rules[1].id, "future-rule");
}

#[test]
fn enable_channel_rejects_invalid_header() {
    let mut section = HeaderInjectionSection::default();
    assert!(section.enable_channel("trae", Some("bad header")).is_err());
    assert!(section.channels.is_empty());
}

#[test]
fn enable_channel_requires_channel_id() {
    let mut section = HeaderInjectionSection::default();
    assert!(section.enable_channel("  ", None).is_err());
}

#[test]
fn disable_channel_keeps_record() {
    let mut section = section_with(true, "X-Opencode-Session");
    assert!(section.disable_channel("trae"));
    let channel = section.channels.get("trae").expect("record kept");
    assert!(!channel.enabled);
    assert_eq!(channel.rules[0].target_header, "X-Opencode-Session");

    assert!(!section.disable_channel("missing"));
}
