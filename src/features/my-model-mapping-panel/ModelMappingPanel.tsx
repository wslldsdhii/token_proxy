import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ChevronsDownUp, ChevronsUpDown, Copy, ListTree, Search } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { m } from "@/paraglide/messages.js";
import { cn } from "@/lib/utils";
import { parseError } from "@/lib/error";

import { persistMappingPanelColumns, readMappingPanelData } from "./api";
import {
  buildRows,
  collectUpstreamOptions,
  filterRows,
  fuzzyMatch,
  nextSortDirection,
  normalizeHiddenColumns,
  sortRows,
} from "./logic";
import type { HidableColumnId, MappingPanelData, SortDirection, SortableColumnId } from "./types";

/**
 * 展开体固定高度：默认窗口（1064×658）扣除接入地址栏 / 折叠头 / 边距后的
 * 设计期常量——不随用户缩放窗口变化（spec：固定高度布局）。
 */
const EXPANDED_BODY_HEIGHT_PX = 520;

const DEFAULT_COLUMN_WIDTHS: Record<string, number> = {
  index: 52,
  alias: 230,
  upstream: 170,
  target: 280,
  providers: 220,
  copy: 72,
};

export function ModelMappingPanel() {
  const [expanded, setExpanded] = useState(false);
  const [data, setData] = useState<MappingPanelData | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [search, setSearch] = useState("");
  const [selectedUpstreams, setSelectedUpstreams] = useState<string[]>([]);
  const [onlyEnabled, setOnlyEnabled] = useState(true);
  const [upstreamMenuSearch, setUpstreamMenuSearch] = useState("");
  const [sortColumn, setSortColumn] = useState<SortableColumnId | null>(null);
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [hiddenColumns, setHiddenColumns] = useState<string[]>([]);
  const [columnWidths, setColumnWidths] = useState<Record<string, number>>({});
  const dragState = useRef<{ column: string; startX: number; startWidth: number } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await readMappingPanelData();
      setData(next);
      setHiddenColumns(normalizeHiddenColumns(next.hiddenColumns));
      setLoadError(false);
    } catch (error) {
      console.debug("[my-model-mapping-panel] load failed", parseError(error));
      setLoadError(true);
    }
  }, []);

  // 每次展开重新读配置快照（面板只读；配置页保存后回仪表盘即见新值）。
  useEffect(() => {
    if (expanded) void refresh();
  }, [expanded, refresh]);

  useEffect(() => {
    const onMove = (event: MouseEvent) => {
      const drag = dragState.current;
      if (!drag) return;
      const delta = event.clientX - drag.startX;
      const next = Math.max(48, drag.startWidth + delta);
      setColumnWidths((prev) => ({ ...prev, [drag.column]: next }));
    };
    const onUp = () => {
      dragState.current = null;
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    return () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
  }, []);

  const startColumnResize = (column: string) => (event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    dragState.current = {
      column,
      startX: event.clientX,
      startWidth: columnWidths[column] ?? DEFAULT_COLUMN_WIDTHS[column] ?? 120,
    };
  };

  const rows = useMemo(() => buildRows(data?.upstreams ?? []), [data]);
  const upstreamOptions = useMemo(
    () => collectUpstreamOptions(data?.upstreams ?? [], onlyEnabled),
    [data, onlyEnabled]
  );
  const visibleRows = useMemo(
    () =>
      sortRows(filterRows(rows, search, selectedUpstreams, onlyEnabled), sortColumn, sortDirection),
    [rows, search, selectedUpstreams, onlyEnabled, sortColumn, sortDirection]
  );
  const filteredUpstreamOptions = useMemo(
    () => upstreamOptions.filter((option) => fuzzyMatch(option, upstreamMenuSearch)),
    [upstreamOptions, upstreamMenuSearch]
  );

  const copyText = useCallback(async (text: string) => {
    try {
      await writeText(text);
      toast.success(m.my_model_mapping_panel_copy_success());
    } catch (error) {
      console.debug("[my-model-mapping-panel] copy failed", parseError(error));
      toast.error(m.my_model_mapping_panel_copy_failed());
    }
  }, []);

  const clearFilters = () => {
    setSearch("");
    setSelectedUpstreams([]);
    setUpstreamMenuSearch("");
  };
  const hasActiveFilters = search.trim().length > 0 || selectedUpstreams.length > 0;

  const toggleColumn = (column: HidableColumnId, checked: boolean) => {
    // 至少保留一个数据列：取消最后一列的操作被拒绝。
    const next = checked
      ? hiddenColumns.filter((item) => item !== column)
      : [...hiddenColumns, column];
    const visibleDataColumns = ["alias", "upstream", "target", "providers"].filter(
      (item) => !next.includes(item)
    );
    if (visibleDataColumns.length === 0) return;
    setHiddenColumns(next);
    void persistMappingPanelColumns(next).catch((error) => {
      console.debug("[my-model-mapping-panel] persist columns failed", parseError(error));
      toast.error(m.my_model_mapping_panel_persist_failed());
    });
  };

  const toggleSort = (column: SortableColumnId) => {
    const next = nextSortDirection(sortColumn === column ? column : null, sortDirection);
    // 同列切换方向；新列从升序开始。
    if (sortColumn === column) {
      setSortDirection(next.direction);
    } else {
      setSortColumn(column);
      setSortDirection("asc");
    }
  };

  const widthOf = (column: string) => columnWidths[column] ?? DEFAULT_COLUMN_WIDTHS[column] ?? 120;
  const isHidden = (column: HidableColumnId) => hiddenColumns.includes(column);

  const sortableHeader = (column: SortableColumnId, label: string) => (
    <th
      className="h-9 select-none px-3 text-left align-middle text-xs font-medium text-muted-foreground"
      style={{ width: widthOf(column) }}
    >
      <div className="flex items-center gap-1">
        <button
          type="button"
          className={cn("hover:text-foreground", sortColumn === column && "text-foreground")}
          onClick={() => toggleSort(column)}
          data-testid={`mapping-sort-${column}`}
        >
          {label}
          {sortColumn === column ? (sortDirection === "asc" ? " ↑" : " ↓") : ""}
        </button>
        <span
          aria-hidden="true"
          className="ml-auto h-4 w-1 cursor-col-resize rounded bg-border/70 hover:bg-primary/60"
          onMouseDown={startColumnResize(column)}
        />
      </div>
    </th>
  );

  const copyableCell = (column: string, text: string, className?: string) => (
    <td
      className={cn(
        "cursor-pointer px-3 py-2 align-middle text-xs",
        "hover:bg-muted/60",
        className
      )}
      style={{ width: widthOf(column) }}
      title={text}
      onClick={() => void copyText(text)}
      data-testid={`mapping-cell-${column}`}
    >
      <span className="block truncate">{text}</span>
    </td>
  );

  return (
    <TooltipProvider>
      <div className="px-4 lg:px-6" data-testid="my-model-mapping-panel">
        <Card className="gap-0 border-border/60 bg-background/70 py-0">
          <CardContent className="flex flex-col gap-0 py-2.5">
            <div className="flex items-center gap-2">
              <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                <ListTree className="size-3.5" aria-hidden="true" />
              </span>
              <span className="text-xs font-medium whitespace-nowrap">
                {m.my_model_mapping_panel_title()}
              </span>
              <span className="hidden truncate text-xs text-muted-foreground md:inline">
                {m.my_model_mapping_panel_desc()}
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto h-7 gap-1 px-2 text-xs"
                onClick={() => setExpanded((prev) => !prev)}
                data-testid="mapping-panel-toggle"
              >
                {expanded ? (
                  <ChevronsDownUp className="size-3.5" aria-hidden="true" />
                ) : (
                  <ChevronsUpDown className="size-3.5" aria-hidden="true" />
                )}
                {expanded
                  ? m.my_model_mapping_panel_collapse()
                  : m.my_model_mapping_panel_expand()}
              </Button>
            </div>

            {expanded ? (
              <div
                className="mt-2 flex flex-col gap-2"
                style={{ height: EXPANDED_BODY_HEIGHT_PX }}
                data-testid="mapping-panel-body"
              >
                {loadError ? (
                  <div className="flex flex-1 items-center justify-center text-xs text-muted-foreground">
                    {m.my_model_mapping_panel_load_failed()}
                  </div>
                ) : (
                  <>
                    <div className="flex flex-wrap items-center gap-2">
                      <label
                        className="flex shrink-0 cursor-pointer select-none items-center gap-1.5 text-xs text-muted-foreground"
                        data-testid="mapping-only-enabled"
                      >
                        <Checkbox
                          checked={onlyEnabled}
                          onCheckedChange={(checked) => setOnlyEnabled(checked === true)}
                          aria-label={m.my_model_mapping_panel_only_enabled()}
                        />
                        {m.my_model_mapping_panel_only_enabled()}
                      </label>
                      <div className="relative min-w-0 flex-1 sm:max-w-xs">
                        <Search
                          className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
                          aria-hidden="true"
                        />
                        <Input
                          value={search}
                          onChange={(event) => setSearch(event.target.value)}
                          placeholder={m.my_model_mapping_panel_search_placeholder()}
                          className="h-8 pl-7 text-xs"
                          data-testid="mapping-search-input"
                        />
                      </div>

                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-8 gap-1 text-xs"
                            data-testid="mapping-upstream-filter"
                          >
                            {selectedUpstreams.length > 0
                              ? m.my_model_mapping_panel_filter_selected({
                                  count: selectedUpstreams.length,
                                })
                              : m.my_model_mapping_panel_filter_upstream()}
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="start" className="w-56">
                          <div className="p-1" onKeyDown={(e) => e.stopPropagation()}>
                            <Input
                              value={upstreamMenuSearch}
                              onChange={(event) => setUpstreamMenuSearch(event.target.value)}
                              onClick={(e) => e.stopPropagation()}
                              placeholder={m.my_model_mapping_panel_filter_search()}
                              className="h-7 text-xs"
                              data-testid="mapping-upstream-menu-search"
                            />
                          </div>
                          {filteredUpstreamOptions.length === 0 ? (
                            <div className="px-2 py-1.5 text-xs text-muted-foreground">
                              {m.my_model_mapping_panel_filter_empty()}
                            </div>
                          ) : (
                            filteredUpstreamOptions.map((option) => (
                              <DropdownMenuCheckboxItem
                                key={option}
                                checked={selectedUpstreams.includes(option)}
                                onCheckedChange={(checked) =>
                                  setSelectedUpstreams((prev) =>
                                    checked
                                      ? [...prev, option]
                                      : prev.filter((item) => item !== option)
                                  )
                                }
                                onSelect={(event) => event.preventDefault()}
                                className="text-xs"
                              >
                                {option}
                              </DropdownMenuCheckboxItem>
                            ))
                          )}
                        </DropdownMenuContent>
                      </DropdownMenu>

                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-8 gap-1 text-xs"
                            data-testid="mapping-columns-toggle"
                          >
                            {m.my_model_mapping_panel_columns()}
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-44">
                          {(
                            [
                              ["alias", m.my_model_mapping_panel_col_alias()],
                              ["upstream", m.my_model_mapping_panel_col_upstream()],
                              ["target", m.my_model_mapping_panel_col_target()],
                              ["providers", m.my_model_mapping_panel_col_providers()],
                            ] as const
                          ).map(([column, label]) => (
                            <DropdownMenuCheckboxItem
                              key={column}
                              checked={!isHidden(column)}
                              onCheckedChange={(checked) => toggleColumn(column, checked)}
                              onSelect={(event) => event.preventDefault()}
                              className="text-xs"
                            >
                              {label}
                            </DropdownMenuCheckboxItem>
                          ))}
                        </DropdownMenuContent>
                      </DropdownMenu>

                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-8 px-2 text-xs"
                            disabled={!hasActiveFilters}
                            onClick={clearFilters}
                            data-testid="mapping-clear-filters"
                          >
                            {m.my_model_mapping_panel_clear()}
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>{m.my_model_mapping_panel_clear_tooltip()}</TooltipContent>
                      </Tooltip>
                    </div>

                    <div className="min-h-0 flex-1 overflow-auto rounded-md border border-border/60 bg-background/60">
                      {visibleRows.length === 0 ? (
                        <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
                          {rows.length === 0
                            ? m.my_model_mapping_panel_empty()
                            : m.my_model_mapping_panel_empty_filtered()}
                        </div>
                      ) : (
                        <table className="w-full border-collapse text-sm">
                          <thead className="sticky top-0 z-10 bg-background/95 backdrop-blur">
                            <tr className="border-b border-border/60">
                              <th
                                className="h-9 px-2 text-left align-middle text-xs font-medium text-muted-foreground"
                                style={{ width: widthOf("index") }}
                              >
                                #
                              </th>
                              {!isHidden("alias") && sortableHeader("alias", m.my_model_mapping_panel_col_alias())}
                              {!isHidden("upstream") &&
                                sortableHeader("upstream", m.my_model_mapping_panel_col_upstream())}
                              {!isHidden("target") &&
                                sortableHeader("target", m.my_model_mapping_panel_col_target())}
                              {!isHidden("providers") && (
                                <th
                                  className="h-9 px-3 text-left align-middle text-xs font-medium text-muted-foreground"
                                  style={{ width: widthOf("providers") }}
                                >
                                  <div className="flex items-center">
                                    <button
                                      type="button"
                                      className={cn(
                                        "select-none hover:text-foreground",
                                        sortColumn === "providers" && "text-foreground"
                                      )}
                                      onClick={() => toggleSort("providers")}
                                    >
                                      {m.my_model_mapping_panel_col_providers()}
                                      {sortColumn === "providers"
                                        ? sortDirection === "asc"
                                          ? " ↑"
                                          : " ↓"
                                        : ""}
                                    </button>
                                    <span
                                      aria-hidden="true"
                                      className="ml-auto h-4 w-1 cursor-col-resize rounded bg-border/70 hover:bg-primary/60"
                                      onMouseDown={startColumnResize("providers")}
                                    />
                                  </div>
                                </th>
                              )}
                              <th
                                className="h-9 px-2 text-center align-middle text-xs font-medium text-muted-foreground"
                                style={{ width: widthOf("copy") }}
                              >
                                {m.my_model_mapping_panel_col_copy()}
                              </th>
                            </tr>
                          </thead>
                          <tbody>
                            {visibleRows.map((row, index) => (
                              <tr
                                key={`${row.upstream}:${row.alias}`}
                                className="border-b border-border/40 last:border-b-0 hover:bg-muted/30"
                              >
                                <td className="px-2 py-2 text-center align-middle text-xs text-muted-foreground tabular-nums">
                                  {index + 1}
                                </td>
                                {!isHidden("alias") && copyableCell("alias", row.alias, "font-medium text-foreground")}
                                {!isHidden("upstream") && copyableCell("upstream", row.upstream)}
                                {!isHidden("target") && copyableCell("target", row.target)}
                                {!isHidden("providers") && (
                                  <td
                                    className="px-3 py-2 align-middle"
                                    style={{ width: widthOf("providers") }}
                                  >
                                    <div className="flex flex-wrap gap-1">
                                      {row.providers.map((provider) => (
                                        <Badge
                                          key={provider}
                                          variant="outline"
                                          className="px-1 py-0 text-[10px] leading-4"
                                        >
                                          {provider}
                                        </Badge>
                                      ))}
                                    </div>
                                  </td>
                                )}
                                <td className="px-2 py-2 text-center align-middle">
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    className="size-6 p-0"
                                    onClick={() => void copyText(row.alias)}
                                    aria-label={m.my_model_mapping_panel_col_copy()}
                                    data-testid={`mapping-copy-${index}`}
                                  >
                                    <Copy className="size-3.5" aria-hidden="true" />
                                  </Button>
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      )}
                    </div>
                  </>
                )}
              </div>
            ) : null}
          </CardContent>
        </Card>
      </div>
    </TooltipProvider>
  );
}
