import { describe, expect, it } from "vitest";

import {
  buildRows,
  collectUpstreamOptions,
  filterRows,
  fuzzyMatch,
  nextSortDirection,
  normalizeHiddenColumns,
  sortRows,
} from "./logic";
import type { MappingPanelUpstream } from "./types";

const upstreams: MappingPanelUpstream[] = [
  {
    id: "glm-proxy",
    providers: ["openai"],
    mappings: [
      { pattern: "glm5", target: "glm-5.3" },
      { pattern: "glm5-air", target: "glm-5.3-air" },
    ],
  },
  { id: "no-mapping", providers: ["openai"], mappings: [] },
  {
    id: "kimi",
    providers: ["openai", "anthropic"],
    mappings: [{ pattern: "k2", target: "kimi-k2-0905" }],
  },
];

describe("buildRows / collectUpstreamOptions", () => {
  it("仅展开有映射的渠道，空映射渠道不出现", () => {
    const rows = buildRows(upstreams);
    expect(rows).toHaveLength(3);
    expect(rows.map((row) => row.upstream)).toEqual(["glm-proxy", "glm-proxy", "kimi"]);
    expect(rows[0]).toMatchObject({ alias: "glm5", target: "glm-5.3", providers: ["openai"] });
    expect(collectUpstreamOptions(upstreams)).toEqual(["glm-proxy", "kimi"]);
  });
});

describe("filterRows", () => {
  const rows = buildRows(upstreams);

  it("搜索词对映射名与模型 id 不区分大小写模糊匹配", () => {
    expect(filterRows(rows, "GLM5", [])).toHaveLength(2);
    expect(filterRows(rows, "KIMI-K2", [])).toHaveLength(1);
    expect(filterRows(rows, "no-hit", [])).toHaveLength(0);
  });

  it("渠道多选之间 OR，与搜索词 AND", () => {
    expect(filterRows(rows, "", ["kimi"])).toHaveLength(1);
    expect(filterRows(rows, "", ["glm-proxy", "kimi"])).toHaveLength(3);
    expect(filterRows(rows, "air", ["glm-proxy", "kimi"])).toHaveLength(1);
    expect(filterRows(rows, "kimi", ["glm-proxy"])).toHaveLength(0);
  });
});

describe("sortRows", () => {
  const rows = buildRows(upstreams);

  it("按映射名列升降序切换，编号与排序解耦", () => {
    const asc = sortRows(rows, "alias", "asc");
    expect(asc.map((row) => row.alias)).toEqual(["glm5", "glm5-air", "k2"]);
    const desc = sortRows(rows, "alias", "desc");
    expect(desc.map((row) => row.alias)).toEqual(["k2", "glm5-air", "glm5"]);
  });

  it("默认序保持配置顺序", () => {
    expect(sortRows(rows, null, "asc").map((row) => row.alias)).toEqual([
      "glm5",
      "glm5-air",
      "k2",
    ]);
  });
});

describe("normalizeHiddenColumns", () => {
  it("过滤未知列名并去重", () => {
    expect(normalizeHiddenColumns(["target", "unknown", "target", "alias"])).toEqual([
      "target",
      "alias",
    ]);
  });
});

describe("nextSortDirection", () => {
  it("null → 首列升序；同列在升/降间循环", () => {
    expect(nextSortDirection(null, "asc")).toEqual({ column: "alias", direction: "asc" });
    expect(nextSortDirection("alias", "asc")).toEqual({ column: "alias", direction: "desc" });
    expect(nextSortDirection("alias", "desc")).toEqual({ column: "alias", direction: "asc" });
  });
});

describe("渠道启用状态过滤", () => {
  const mixed: MappingPanelUpstream[] = [
    {
      id: "on-a",
      enabled: true,
      providers: ["openai"],
      mappings: [{ pattern: "p1", target: "t1" }],
    },
    {
      id: "off-b",
      enabled: false,
      providers: ["openai"],
      mappings: [{ pattern: "p2", target: "t2" }],
    },
    { id: "legacy-c", providers: ["openai"], mappings: [{ pattern: "p3", target: "t3" }] },
  ];

  it("默认仅启用：禁用渠道的行与菜单项不出现", () => {
    const rows = buildRows(mixed);
    expect(filterRows(rows, "", [], true).map((r) => r.upstream)).toEqual(["on-a", "legacy-c"]);
    expect(collectUpstreamOptions(mixed, true)).toEqual(["on-a", "legacy-c"]);
  });

  it("取消勾选显示全部（含禁用）", () => {
    const rows = buildRows(mixed);
    expect(filterRows(rows, "", [], false).map((r) => r.upstream)).toEqual([
      "on-a",
      "off-b",
      "legacy-c",
    ]);
    expect(collectUpstreamOptions(mixed, false)).toEqual(["on-a", "off-b", "legacy-c"]);
  });

  it("与搜索组合：禁用渠道被启用过滤先行排除", () => {
    const rows = buildRows(mixed);
    expect(filterRows(rows, "p2", [], true)).toHaveLength(0);
    expect(filterRows(rows, "p2", [], false).map((r) => r.upstream)).toEqual(["off-b"]);
  });

  it("旧 DTO 缺省 enabled 字段按启用处理", () => {
    expect(buildRows(mixed).find((r) => r.upstream === "legacy-c")?.enabled).toBe(true);
  });
});

describe("fuzzyMatch", () => {
  it("空查询恒匹配", () => {
    expect(fuzzyMatch("anything", "")).toBe(true);
    expect(fuzzyMatch("anything", "   ")).toBe(true);
  });
});
