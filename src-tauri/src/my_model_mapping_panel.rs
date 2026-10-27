//! my_model_mapping_panel —— 仪表盘「模型映射」面板的命令层（fork 本地增强）。
//!
//! 与 config 层 `token_proxy_config::my_model_mapping_panel` 的分工：
//! - 本文件：读配置装配面板 DTO（仅含**有映射配置**的渠道）、列显示持久化、
//!   对前端暴露两个命令；
//! - config 层：段结构与容错反序列化（纯逻辑，已单测）。
//!
//! 数据源是本地已保存的渠道配置（`config.jsonc` 的 `upstreams[].model_mappings`），
//! 不做任何模型 API 请求。

use std::sync::Arc;

use tauri::Manager;

use token_proxy_account_store::paths::TokenProxyPaths;

type PathsState = Arc<TokenProxyPaths>;

/// 一条映射（pattern → target）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MappingPanelEntry {
    pattern: String,
    target: String,
}

/// 一个渠道的映射集合（仅含 model_mappings 非空的渠道）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MappingPanelUpstream {
    id: String,
    providers: Vec<String>,
    mappings: Vec<MappingPanelEntry>,
}

/// 面板数据：渠道映射 + 持久化的隐藏列。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MappingPanelData {
    upstreams: Vec<MappingPanelUpstream>,
    hidden_columns: Vec<String>,
}

/// 列显示持久化允许的列 id（未知列名读取时被过滤，写入口径一致）。
const KNOWN_COLUMNS: [&str; 4] = ["alias", "upstream", "target", "providers"];

fn normalize_hidden_columns(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .filter(|column| KNOWN_COLUMNS.contains(&column.as_str()) && seen.insert(column.clone()))
        .collect::<Vec<_>>()
}

#[tauri::command]
pub(crate) async fn model_mapping_panel_data(
    app: tauri::AppHandle,
) -> Result<MappingPanelData, String> {
    let paths = app.state::<PathsState>().inner().clone();
    let config = token_proxy_config::read_config(paths.as_ref()).await?.config;
    let upstreams = config
        .upstreams
        .iter()
        .filter(|upstream| !upstream.model_mappings.is_empty())
        .map(|upstream| MappingPanelUpstream {
            id: upstream.id.clone(),
            providers: upstream.providers.clone(),
            mappings: upstream
                .model_mappings
                .iter()
                .map(|(pattern, target)| MappingPanelEntry {
                    pattern: pattern.clone(),
                    target: target.clone(),
                })
                .collect(),
        })
        .collect();
    let hidden_columns = config
        .my_model_mapping_panel
        .as_ref()
        .map(|section| normalize_hidden_columns(section.hidden_columns.iter().cloned()))
        .unwrap_or_default();
    Ok(MappingPanelData {
        upstreams,
        hidden_columns,
    })
}

/// 持久化列显示状态：只替换 `my_model_mapping_panel` 段，不触发代理 reload。
#[tauri::command]
pub(crate) async fn model_mapping_panel_set_columns(
    app: tauri::AppHandle,
    hidden_columns: Vec<String>,
) -> Result<(), String> {
    let paths = app.state::<PathsState>().inner().clone();
    let mut config = token_proxy_config::read_config(paths.as_ref()).await?.config;
    let hidden_columns = normalize_hidden_columns(hidden_columns);
    if hidden_columns.is_empty() {
        // 全显 = 移除段字段（skip 序列化语义下等价于缺省全显）。
        config.my_model_mapping_panel = None;
    } else {
        config.my_model_mapping_panel =
            Some(token_proxy_config::my_model_mapping_panel::MyModelMappingPanelSection {
                hidden_columns,
            });
    }
    token_proxy_config::write_config(paths.as_ref(), config).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_hidden_columns;

    #[test]
    fn normalizes_dedupes_and_drops_unknown_columns() {
        assert_eq!(
            normalize_hidden_columns([
                "target".to_string(),
                "unknown".to_string(),
                "target".to_string(),
                "alias".to_string(),
            ]),
            vec!["target".to_string(), "alias".to_string()]
        );
        assert!(normalize_hidden_columns(Vec::new()).is_empty());
    }
}
