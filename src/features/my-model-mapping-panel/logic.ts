import type {
  HidableColumnId,
  MappingPanelUpstream,
  MappingRow,
  SortableColumnId,
  SortDirection,
} from "./types";

/** 不区分大小写的子串匹配（映射名 / 模型 id / 渠道筛选项共用）。 */
export function fuzzyMatch(value: string, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return value.toLowerCase().includes(needle);
}

/** 配置 → 行集合：仅展开有映射的渠道（后端已过滤空渠道，此处防御性再过滤一次）。 */
export function buildRows(upstreams: readonly MappingPanelUpstream[]): MappingRow[] {
  const rows: MappingRow[] = [];
  for (const upstream of upstreams) {
    if (upstream.mappings.length === 0) continue;
    for (const mapping of upstream.mappings) {
      if (!mapping.pattern) continue;
      rows.push({
        alias: mapping.pattern,
        upstream: upstream.id,
        target: mapping.target,
        providers: upstream.providers,
      });
    }
  }
  return rows;
}

/** 渠道筛选选项：仅含有映射配置的渠道（与行集合同源）。 */
export function collectUpstreamOptions(upstreams: readonly MappingPanelUpstream[]): string[] {
  return upstreams.filter((u) => u.mappings.length > 0).map((u) => u.id);
}

/** 组合筛选：搜索词命中（alias OR target）AND 渠道多选（OR，空选=全部）。 */
export function filterRows(
  rows: readonly MappingRow[],
  search: string,
  selectedUpstreams: readonly string[]
): MappingRow[] {
  const selected = new Set(selectedUpstreams);
  return rows.filter((row) => {
    if (selected.size > 0 && !selected.has(row.upstream)) return false;
    if (!fuzzyMatch(row.alias, search) && !fuzzyMatch(row.target, search)) return false;
    return true;
  });
}

function compareText(left: string, right: string): number {
  return left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
}

/** 排序：列内文本比较（providers 列按首个标签），编号列不参与（始终行序 1..n）。 */
export function sortRows(
  rows: readonly MappingRow[],
  column: SortableColumnId | null,
  direction: SortDirection
): MappingRow[] {
  const sorted = [...rows];
  if (!column) return sorted;
  const key = (row: MappingRow): string => {
    switch (column) {
      case "alias":
        return row.alias;
      case "upstream":
        return row.upstream;
      case "target":
        return row.target;
      case "providers":
        return row.providers.join(",");
    }
  };
  sorted.sort((left, right) => {
    const order = compareText(key(left), key(right));
    return direction === "asc" ? order : -order;
  });
  return sorted;
}

/**
 * 隐藏列规范化：过滤未知列名、去重。返回值不可使可隐藏列全部隐藏
 * （至少保留一列的约束在 UI 交互层拦截，此处仅做读取侧清洗）。
 */
export function normalizeHiddenColumns(raw: readonly string[]): HidableColumnId[] {
  const known = new Set<string>(["alias", "upstream", "target", "providers"]);
  const seen = new Set<string>();
  const output: HidableColumnId[] = [];
  for (const column of raw) {
    if (known.has(column) && !seen.has(column)) {
      seen.add(column);
      output.push(column as HidableColumnId);
    }
  }
  return output;
}

/** 下一排序态：null（默认序）→ asc → desc → asc …（两方循环，回到升序不回默认）。 */
export function nextSortDirection(
  column: SortableColumnId | null,
  direction: SortDirection
): { column: SortableColumnId; direction: SortDirection } {
  if (column === null) {
    return { column: "alias", direction: "asc" };
  }
  return {
    column,
    direction: direction === "asc" ? ("desc" as const) : ("asc" as const),
  };
}
