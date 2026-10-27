//! my_model_mapping_panel —— 仪表盘「模型映射」面板的列显示持久化段（fork 本地增强）。
//!
//! 段只存列显示状态（隐藏列集合）；面板的展开状态、搜索词、渠道筛选、
//! 排序、列宽均为会话内状态，不落盘。缺段/字段缺失 = 四个数据列全部显示；
//! 未知列名由前端读取时过滤（旧版本列名演进不致显示异常）。

use serde::{Deserialize, Serialize};

/// `config.jsonc` 顶层 `my_model_mapping_panel` 段。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MyModelMappingPanelSection {
    /// 隐藏的数据列（可选项：alias / upstream / target / providers）；
    /// 缺省或空 = 全部显示。编号列与复制按钮列恒显、不可隐藏。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_columns: Vec<String>,
}

/// 容错反序列化：形态非法时按「段不存在」处理（照 `my_proxy_endpoint::de_lenient`），
/// 用户手改配置写坏该段不致 GUI 与 CLI 加载失败。
pub fn de_lenient<'de, D>(deserializer: D) -> Result<Option<MyModelMappingPanelSection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if !value.is_object() {
        if !value.is_null() {
            tracing::warn!("my_model_mapping_panel section ignored: expected an object");
        }
        return Ok(None);
    }
    match serde_json::from_value::<MyModelMappingPanelSection>(value) {
        Ok(section) => Ok(Some(section)),
        Err(err) => {
            tracing::warn!(error = %err, "my_model_mapping_panel section ignored: invalid shape");
            Ok(None)
        }
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
