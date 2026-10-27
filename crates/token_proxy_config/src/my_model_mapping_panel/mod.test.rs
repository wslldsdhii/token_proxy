use super::*;

#[test]
fn 段缺省全显_空集等价() {
    assert!(MyModelMappingPanelSection::default().hidden_columns.is_empty());
    let parsed: MyModelMappingPanelSection =
        serde_json::from_str("{}").expect("empty object");
    assert!(parsed.hidden_columns.is_empty());
}

#[test]
fn 带段往返保留() {
    let text = r#"{"my_model_mapping_panel":{"hidden_columns":["providers","target"]}}"#;
    #[derive(serde::Deserialize)]
    struct Wrapper {
        my_model_mapping_panel: Option<MyModelMappingPanelSection>,
    }
    let parsed: Wrapper = serde_json::from_str(text).expect("parse");
    assert_eq!(
        parsed.my_model_mapping_panel.expect("section").hidden_columns,
        vec!["providers".to_string(), "target".to_string()]
    );
    // 写回：空集合省略段（与 skip_serializing_if 语义一致）。
    let full = MyModelMappingPanelSection {
        hidden_columns: vec!["alias".to_string()],
    };
    let text = serde_json::to_string(&full).expect("serialize");
    assert!(text.contains("alias"));
    let empty = MyModelMappingPanelSection::default();
    let text = serde_json::to_string(&empty).expect("serialize");
    assert_eq!(text, "{}", "空集合不写字段");
}

#[test]
fn 容错反序列化_非法形态回落缺段() {
    let bad = serde_json::from_str::<Option<serde_json::Value>>(r#""oops""#)
        .expect("value")
        .unwrap();
    // 直接走 de_lenient 的核心路径（serde_json::Value 形态）。
    let section: Option<MyModelMappingPanelSection> = match bad {
        ref value if value.is_object() => serde_json::from_value(value.clone()).ok(),
        _ => None,
    };
    assert!(section.is_none(), "字符串形态回落缺段");
}
