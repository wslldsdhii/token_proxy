import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

import { ModelMappingPanel } from "./ModelMappingPanel";
import type { MappingPanelData } from "./types";

const { readMock, persistMock, writeTextMock } = vi.hoisted(() => ({
  readMock: vi.fn(),
  persistMock: vi.fn(),
  writeTextMock: vi.fn(),
}));

vi.mock("./api", () => ({
  readMappingPanelData: readMock,
  persistMappingPanelColumns: persistMock,
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: writeTextMock,
}));

function panelData(overrides: Partial<MappingPanelData> = {}): MappingPanelData {
  return {
    upstreams: [
      {
        id: "glm-proxy",
        providers: ["openai"],
        mappings: [
          { pattern: "glm5", target: "glm-5.3" },
          { pattern: "glm5-air", target: "glm-5.3-air" },
        ],
      },
      {
        id: "kimi",
        providers: ["openai", "anthropic"],
        mappings: [{ pattern: "k2", target: "kimi-k2" }],
      },
    ],
    hiddenColumns: [],
    ...overrides,
  };
}

function renderPanel() {
  return render(
    <I18nProvider>
      <ModelMappingPanel />
    </I18nProvider>
  );
}

async function expand() {
  await userEvent.click(screen.getByTestId("mapping-panel-toggle"));
  await waitFor(() => expect(screen.getByTestId("mapping-panel-body")).toBeTruthy());
}

describe("my-model-mapping-panel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    readMock.mockResolvedValue(panelData());
    persistMock.mockResolvedValue(undefined);
    writeTextMock.mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  it("默认折叠，展开后加载数据并渲染映射行（编号 1..n）", async () => {
    renderPanel();
    expect(screen.queryByTestId("mapping-panel-body")).toBeNull();

    await expand();

    expect(readMock).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("glm5")).toBeTruthy();
    expect(screen.getByText("k2")).toBeTruthy();
    const indexCells = screen.getAllByText(/^[123]$/);
    expect(indexCells).toHaveLength(3); // 编号连续 1..3
  });

  it("搜索词过滤映射名与模型 id；清空恢复", async () => {
    renderPanel();
    await expand();
    await screen.findByText("glm5");

    await userEvent.type(screen.getByTestId("mapping-search-input"), "KIMI-K2");
    await waitFor(() => expect(screen.getByText("k2")).toBeTruthy());
    expect(screen.queryByText("glm5")).toBeNull();

    await userEvent.clear(screen.getByTestId("mapping-search-input"));
    await waitFor(() => expect(screen.getByText("glm5")).toBeTruthy());
  });

  it("列显示切换持久化隐藏列，且至少保留一列", async () => {
    renderPanel();
    await expand();
    await screen.findByText("glm5");

    // 打开列控制菜单
    await userEvent.click(screen.getByTestId("mapping-columns-toggle"));
    const aliasOption = await screen.findByRole("menuitemcheckbox", { name: /映射名|Alias/ });
    await userEvent.click(aliasOption);
    await waitFor(() => expect(persistMock).toHaveBeenCalledWith(["alias"]));

    // 隐藏所有四列的尝试被拒绝：连续取消剩余列后，persist 不再收到全隐藏集合。
    persistMock.mockClear();
    const remaining = await screen.findAllByRole("menuitemcheckbox");
    for (const option of remaining) {
      const isChecked = option.getAttribute("data-state") === "checked";
      if (isChecked && option !== aliasOption) {
        await userEvent.click(option);
      }
    }
    const calls = persistMock.mock.calls as string[][];
    const lastCall = calls[calls.length - 1];
    expect(lastCall.filter((c) => c !== "alias").length).toBeLessThan(4);
    expect(
      ["alias", "upstream", "target", "providers"].filter((c) => !lastCall.includes(c)),
    ).not.toHaveLength(0);
  });

  it("重新展开重新读配置（面板只读、每次展开刷新）", async () => {
    renderPanel();
    await expand();
    await screen.findByText("glm5");
    const toggle = screen.getByTestId("mapping-panel-toggle");
    await userEvent.click(toggle); // 收起
    readMock.mockResolvedValue(
      panelData({ upstreams: [{ id: "only", providers: ["openai"], mappings: [{ pattern: "p1", target: "t1" }] }] }),
    );
    await userEvent.click(toggle); // 再展开
    await waitFor(() => expect(readMock).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText("p1")).toBeTruthy());
  });

  it("「仅启用」默认勾选只显示启用渠道，取消后显示全部", async () => {
    readMock.mockResolvedValue(
      panelData({
        upstreams: [
          { id: "on-a", enabled: true, providers: ["openai"], mappings: [{ pattern: "p1", target: "t1" }] },
          { id: "off-b", enabled: false, providers: ["openai"], mappings: [{ pattern: "p2", target: "t2" }] },
        ],
      }),
    );
    renderPanel();
    await expand();
    await screen.findByText("p1");
    expect(screen.queryByText("p2")).toBeNull(); // 禁用渠道默认不显示

    const toggle = screen.getByTestId("mapping-only-enabled");
    const checkbox = toggle.querySelector("button[role=checkbox]") as HTMLElement;
    await userEvent.click(checkbox);
    // 取消勾选后显示全部
    await waitFor(() => expect(screen.getByText("p2")).toBeTruthy());
  });

  it("「同时复制提供商」默认勾选复制渠道/映射名；取消后只复制映射名", async () => {
    renderPanel();
    await expand();
    await screen.findByText("glm5");

    const label = screen.getByTestId("mapping-copy-provider");
    const checkbox = label.querySelector("button[role=checkbox]") as HTMLElement;
    expect(checkbox.getAttribute("data-state")).toBe("checked");

    await userEvent.click(screen.getAllByTestId("mapping-cell-alias")[0]);
    await waitFor(() => expect(writeTextMock).toHaveBeenCalledWith("glm-proxy/glm5"));

    await userEvent.click(screen.getByTestId("mapping-copy-0"));
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("glm-proxy/glm5"));

    await userEvent.click(checkbox);
    expect(checkbox.getAttribute("data-state")).toBe("unchecked");
    await userEvent.click(screen.getAllByTestId("mapping-cell-alias")[0]);
    await waitFor(() => expect(writeTextMock).toHaveBeenLastCalledWith("glm5"));
  });

  it("持久化的隐藏列在加载后生效（target 列不渲染）", async () => {
    readMock.mockResolvedValue(panelData({ hiddenColumns: ["target"] }));
    renderPanel();
    await expand();
    await screen.findByText("glm5");
    expect(screen.queryByText("glm-5.3")).toBeNull(); // target 列被隐藏
    expect(screen.queryByText("kimi-k2")).toBeNull(); // kimi 的 target 同样不渲染
  });

  it("点击标题行任意位置切换展开/收起", async () => {
    renderPanel();
    await userEvent.click(screen.getByText(m.my_model_mapping_panel_title()));
    expect(await screen.findByTestId("mapping-panel-body")).toBeTruthy();

    await userEvent.click(screen.getByText(m.my_model_mapping_panel_desc()));
    await waitFor(() => expect(screen.queryByTestId("mapping-panel-body")).toBeNull());
  });

  it("标题行支持键盘 Enter 切换", async () => {
    renderPanel();
    const toggle = screen.getByTestId("mapping-panel-toggle");
    toggle.focus();
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByTestId("mapping-panel-body")).toBeTruthy();
  });

  it("点击表头单元格任意区域切换排序", async () => {
    renderPanel();
    await expand();

    const header = screen.getByTestId("mapping-sort-alias");
    await userEvent.click(header);
    expect(header.textContent).toContain("↑");
    await userEvent.click(header);
    expect(header.textContent).toContain("↓");
  });

  it("拖拽列宽手柄不触发排序", async () => {
    renderPanel();
    await expand();

    const header = screen.getByTestId("mapping-sort-alias");
    const handle = screen.getByTestId("mapping-resize-alias");
    fireEvent.mouseDown(handle, { clientX: 100 });
    fireEvent.click(handle);
    fireEvent.mouseUp(document);

    expect(header.textContent).not.toContain("↑");
    expect(header.textContent).not.toContain("↓");
  });
});
