import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/lib/i18n";

import { ModelMappingPanel } from "./ModelMappingPanel";
import type { MappingPanelData } from "./types";

const { readMock, persistMock } = vi.hoisted(() => ({
  readMock: vi.fn(),
  persistMock: vi.fn(),
}));

vi.mock("./api", () => ({
  readMappingPanelData: readMock,
  persistMappingPanelColumns: persistMock,
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

  it("持久化的隐藏列在加载后生效（target 列不渲染）", async () => {
    readMock.mockResolvedValue(panelData({ hiddenColumns: ["target"] }));
    renderPanel();
    await expand();
    await screen.findByText("glm5");
    expect(screen.queryByText("glm-5.3")).toBeNull(); // target 列被隐藏
    expect(screen.queryByText("kimi-k2")).toBeNull(); // kimi 的 target 同样不渲染
  });
});
