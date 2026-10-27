/** 后端 `model_mapping_panel_data` 命令的 DTO（camelCase）。 */
export type MappingPanelEntry = {
  pattern: string;
  target: string;
};

export type MappingPanelUpstream = {
  id: string;
  providers: string[];
  mappings: MappingPanelEntry[];
};

export type MappingPanelData = {
  upstreams: MappingPanelUpstream[];
  hiddenColumns: string[];
};

/** 表格行模型：一条映射一行。 */
export type MappingRow = {
  /** 映射名（配置 model_mappings 的 key）。 */
  alias: string;
  /** 渠道配置名（upstream id）。 */
  upstream: string;
  /** 映射对应的真实请求模型 id。 */
  target: string;
  /** 渠道支持的 api 类型（providers）。 */
  providers: string[];
};

/** 可隐藏数据列 id（编号与复制按钮列恒显）。 */
export const HIDABLE_COLUMN_IDS = ["alias", "upstream", "target", "providers"] as const;
export type HidableColumnId = (typeof HIDABLE_COLUMN_IDS)[number];

export type SortDirection = "asc" | "desc";
export type SortableColumnId = Exclude<HidableColumnId, never>;
