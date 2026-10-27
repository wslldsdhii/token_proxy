import { invoke } from "@tauri-apps/api/core";

import type { MappingPanelData } from "./types";

/** 读取面板数据：仅含有映射配置的渠道 + 持久化的隐藏列。数据源为本地配置。 */
export async function readMappingPanelData(): Promise<MappingPanelData> {
  return invoke<MappingPanelData>("model_mapping_panel_data");
}

/** 持久化列显示状态（只替换 my_model_mapping_panel 段，不触发代理 reload）。 */
export async function persistMappingPanelColumns(hiddenColumns: string[]): Promise<void> {
  return invoke("model_mapping_panel_set_columns", { hiddenColumns });
}
