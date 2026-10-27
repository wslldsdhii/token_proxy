use super::*;

fn config_with(section: Option<RateRecorderSection>) -> crate::ProxyConfigFile {
    let mut config = crate::ProxyConfigFile::default();
    config.rate_recorder = section;
    config
}

#[test]
fn 缺省段视为未启用() {
    let resolved = sync_settings(&config_with(None));
    assert_eq!(resolved, None);
    assert_eq!(current_settings(), None);
}

#[test]
fn 段内缺省与零值语义() {
    assert_eq!(
        resolve_section(&RateRecorderSection::default()),
        RateRecorderResolved {
            enabled: false,
            retention_days: 90,
        }
    );
    // retention_days = 0 视为缺省（防误写导致全量即删）。
    assert_eq!(
        resolve_section(&RateRecorderSection {
            retention_days: Some(0),
            ..RateRecorderSection::default()
        })
        .retention_days,
        90
    );
}

#[test]
fn 同步覆盖_reload生效() {
    sync_settings(&config_with(Some(RateRecorderSection {
        enabled: Some(true),
        retention_days: Some(30),
    })));
    assert_eq!(
        current_settings(),
        Some(RateRecorderResolved {
            enabled: true,
            retention_days: 30,
        })
    );
    // 第二次 sync（reload）真正覆盖旧值。
    sync_settings(&config_with(Some(RateRecorderSection::default())));
    assert_eq!(
        current_settings(),
        Some(RateRecorderResolved {
            enabled: false,
            retention_days: 90,
        })
    );
    // 段移除后回落 None（未启用）。
    sync_settings(&config_with(None));
    assert_eq!(current_settings(), None);
}

#[test]
fn 配置文件序列化_段可省略() {
    let config = config_with(None);
    let text = serde_json::to_string(&config).expect("serialize");
    assert!(!text.contains("rate_recorder"), "缺省不写段");
    let parsed: crate::ProxyConfigFile = serde_json::from_str(&text).expect("deserialize");
    assert!(parsed.rate_recorder.is_none());
}

#[test]
fn 配置文件往返_带段保留() {
    // 模拟 GUI 保存配置：读入 → 整体序列化写回，段不丢（双端写回一致性）。
    let text = r#"{"host":"127.0.0.1","port":9208,"rate_recorder":{"enabled":true,"retention_days":30}}"#;
    let parsed: crate::ProxyConfigFile = serde_json::from_str(text).expect("parse with section");
    assert_eq!(
        resolve_section(parsed.rate_recorder.as_ref().expect("section kept")),
        RateRecorderResolved {
            enabled: true,
            retention_days: 30,
        }
    );
    let rewritten = serde_json::to_string(&parsed).expect("rewrite");
    assert!(rewritten.contains("\"rate_recorder\""), "写回保留段");
    let reparsed: crate::ProxyConfigFile = serde_json::from_str(&rewritten).expect("reparse");
    assert!(reparsed.rate_recorder.is_some(), "再读不丢段");
}
