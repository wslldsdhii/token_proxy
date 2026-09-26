import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UrlComposeEditor } from "@/features/config/cards/upstreams/url-compose-editor";
import { m } from "@/paraglide/messages.js";
import { type UrlComposeConfig } from "@/features/config/types";

afterEach(cleanup);

type SetupOptions = {
  providers?: string[];
  baseUrl?: string;
  value?: UrlComposeConfig;
  prefillSuffixDefaults?: boolean;
};

/** 受控组件需要状态回灌；Harness 持有 state，通过 render-prop 注入 setValue。 */
function setup({
  providers = ["openai", "anthropic"],
  baseUrl = "https://x.com",
  value = {},
  prefillSuffixDefaults,
}: SetupOptions = {}) {
  const onChange = vi.fn();

  function Harness(props: {
    children: (args: {
      value: UrlComposeConfig;
      setValue: (next: UrlComposeConfig) => void;
    }) => ReactNode;
  }) {
    const [state, setState] = useState<UrlComposeConfig>(value);
    return <>{props.children({ value: state, setValue: setState })}</>;
  }

  render(
    <Harness>
      {({ value, setValue }) => (
        <UrlComposeEditor
          providers={providers}
          baseUrl={baseUrl}
          value={value}
          onChange={(next) => {
            onChange(next);
            setValue(next);
          }}
          prefillSuffixDefaults={prefillSuffixDefaults}
        />
      )}
    </Harness>
  );

  return { onChange };
}

async function expand(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByText(m.url_compose_toggle()));
}

describe("config/url-compose-editor", () => {
  it("默认折叠并显示摘要，展开后出现已勾选接口的配置行", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai", "anthropic"] });

    // 折叠态：摘要可见（已勾选家族但未定制 → 均为默认后缀），行未渲染
    expect(screen.getByText(m.url_compose_summary_default())).toBeDefined();
    await expand(user);

    expect(screen.getByText(m.url_compose_family_openai())).toBeDefined();
    expect(screen.getByText(m.url_compose_family_anthropic())).toBeDefined();
    expect(screen.queryByText(m.url_compose_family_openai_response())).toBeNull();
  });

  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (test) START ══════════
  it("勾选 dashscope 出现家族行：/api 示例、默认后缀预填、说明行与空态提示字号", async () => {
    const user = userEvent.setup();
    setup({ providers: ["dashscope"], prefillSuffixDefaults: true });
    await expand(user);

    expect(screen.getByText(m.url_compose_family_dashscope())).toBeDefined();
    expect(screen.getByText(m.url_compose_family_dashscope_tag())).toBeDefined();
    expect(screen.getByText(m.url_compose_dashscope_hint())).toBeDefined();

    const textboxes = screen.getAllByRole("textbox") as HTMLInputElement[];
    expect(textboxes[0].placeholder).toBe(m.url_compose_prefix_placeholder({ example: "/api" }));
    expect(textboxes[0].className).toContain("placeholder:text-[10px]");
    expect(textboxes[1].className).toContain("placeholder:text-[10px]");
    await waitFor(() => {
      expect(textboxes[1].value).toBe("/v1/services/aigc/text-generation/generation");
    });
  });

  it("未勾选 dashscope 不出现家族行与说明行", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai"] });
    await expand(user);

    expect(screen.queryByText(m.url_compose_family_dashscope())).toBeNull();
    expect(screen.queryByText(m.url_compose_dashscope_hint())).toBeNull();
  });
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (test) END ══════════

  it("摘要反映已配置数量", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["openai", "openai-response", "anthropic"],
      value: {
        openai: { prefix: "", suffix: "/v3/chat/completions" },
      },
    });
    expect(screen.getByText(m.url_compose_summary_configured({ count: 1 }))).toBeDefined();
    await expand(user);
  });

  it("后缀失焦自动补开头斜杠", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"] });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1];
    await user.type(suffixInput, "v3/chat/completions");
    await user.tab();

    await waitFor(() => {
      expect((suffixInput as HTMLInputElement).value).toBe("/v3/chat/completions");
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as { openai?: { suffix: string } };
      expect(lastCall.openai?.suffix).toBe("/v3/chat/completions");
    });
  });

  it("前缀输入提示按家族展示示例：openai /openai，anthropic /anthropic", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai", "anthropic"] });
    await expand(user);

    const textboxes = screen.getAllByRole("textbox") as HTMLInputElement[];
    expect(textboxes[0].placeholder).toBe(
      m.url_compose_prefix_placeholder({ example: "/openai" })
    );
    expect(textboxes[2].placeholder).toBe(
      m.url_compose_prefix_placeholder({ example: "/anthropic" })
    );
  });

  it("预览展示 base+prefix+版本段替换后的完整出站地址", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["anthropic"],
      value: { anthropic: { prefix: "/anthropic", suffix: "/v1/messages" } },
    });
    await expand(user);

    const preview = screen.getByText("https://x.com").parentElement;
    expect(preview?.textContent).toContain("/anthropic");
    expect(preview?.textContent).toContain("/v1/messages");
  });

  it("后缀为默认值时重置按钮禁用，修改后可重置", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      value: { openai: { prefix: "", suffix: "/v3/chat/completions" } },
    });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v3/chat/completions");

    const resetTitle = m.url_compose_reset_title({ default: "/v1/chat/completions" });
    const resetButton = screen.getByTitle(resetTitle);
    expect(resetButton.hasAttribute("disabled")).toBe(false);

    await user.click(resetButton);
    await waitFor(() => {
      expect(suffixInput.value).toBe("/v1/chat/completions");
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as { openai?: { suffix: string } };
      expect(lastCall.openai?.suffix).toBe("/v1/chat/completions");
    });
  });

  it("映射示例浮窗展示版本段替换结果", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["anthropic"],
      value: { anthropic: { prefix: "/anthropic", suffix: "/v1/messages" } },
    });
    await expand(user);

    const mapButton = screen.getByRole("button", { name: m.url_compose_map_open() });
    await user.click(mapButton);

    await waitFor(() => {
      expect(
        screen.getByText(m.url_compose_map_title({ name: m.url_compose_family_anthropic() }))
      ).toBeDefined();
    });
    expect(screen.getAllByText("/v1/messages/count_tokens").length).toBeGreaterThan(0);
  });

  it("新建模式预填：已勾选家族后缀自动填入家族默认值", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"], prefillSuffixDefaults: true });

    await waitFor(() => {
      const firstCall = onChange.mock.calls[0]?.[0] as
        | { openai?: { prefix: string; suffix: string } }
        | undefined;
      expect(firstCall?.openai).toEqual({ prefix: "", suffix: "/v1/chat/completions" });
    });

    await expand(user);
    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v1/chat/completions");
  });

  it("编辑模式不预填：未配置 url_compose 的既有渠道后缀保持为空", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"] });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("新建模式手动清空后缀后不被回填", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      prefillSuffixDefaults: true,
      value: { openai: { prefix: "", suffix: "/v1/chat/completions" } },
    });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    await user.clear(suffixInput);
    await user.tab();

    await waitFor(() => {
      expect(suffixInput.value).toBe("");
    });
    // 清空后家族键被删除，预填 effect 不得再次写入默认值
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as {
      openai?: { suffix: string };
    };
    expect(lastCall.openai).toBeUndefined();
  });

  it("复制新建不覆盖已携带的组合配置，仅补缺失家族", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai", "anthropic"],
      prefillSuffixDefaults: true,
      value: { openai: { prefix: "/x", suffix: "/v9/chat/completions" } },
    });

    await waitFor(() => {
      const firstCall = onChange.mock.calls[0]?.[0] as
        | { openai?: { suffix: string }; anthropic?: { suffix: string } }
        | undefined;
      expect(firstCall?.anthropic).toEqual({ prefix: "", suffix: "/v1/messages" });
      expect(firstCall?.openai?.suffix).toBe("/v9/chat/completions");
    });

    await expand(user);
    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v9/chat/completions");
  });

  // ══════════ MY-STRIP-VERSION PATCH 9 (test) START ══════════
  it("每个家族行仅一个复选框（无固定文字），悬浮提示为去除版本号", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai", "anthropic"] });
    await expand(user);

    // 每个家族一个复选框，带 hover title
    const checkboxes = screen.getAllByRole("checkbox", {
      name: m.url_compose_strip_version_title(),
    });
    expect(checkboxes.length).toBe(2);
    expect(screen.getAllByTitle(m.url_compose_strip_version_title()).length).toBe(2);
    // 复选框旁不渲染固定文字（提示只出现在悬浮 title 中）
    expect(screen.queryByText(m.url_compose_strip_version_title())).toBeNull();
  });

  it("勾选去除版本号：版本段红色删除线，取消恢复紫色", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      value: { openai: { prefix: "", suffix: "/v1/chat/completions" } },
    });
    await expand(user);

    // 未勾选：无删除线
    expect(document.body.querySelector(".line-through")).toBeNull();

    const checkbox = screen.getByRole("checkbox", {
      name: m.url_compose_strip_version_title(),
    });
    await user.click(checkbox);

    // 勾选后：镜像层与预览行的版本段带删除线，onChange 携带 strip_version: true
    await waitFor(() => {
      const struck = [...document.body.querySelectorAll(".line-through")];
      expect(struck.map((element) => element.textContent)).toContain("/v1");
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as {
        openai?: { prefix: string; suffix: string; strip_version?: boolean };
      };
      expect(lastCall.openai).toEqual({
        prefix: "",
        suffix: "/v1/chat/completions",
        strip_version: true,
      });
    });

    // 取消勾选恢复（无删除线），strip_version 回落为 false（载荷转换时过滤不落盘）
    await user.click(checkbox);
    await waitFor(() => {
      expect(document.body.querySelector(".line-through")).toBeNull();
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as {
        openai?: { strip_version?: boolean };
      };
      expect(lastCall.openai?.strip_version).toBe(false);
    });
  });

  it("勾选去除版本号后映射浮窗展示去除版本段的最终出站地址", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["anthropic"],
      value: { anthropic: { prefix: "/anthropic", suffix: "/v1/messages", strip_version: true } },
    });
    await expand(user);

    const mapButton = screen.getByRole("button", { name: m.url_compose_map_open() });
    await user.click(mapButton);
    await waitFor(() => {
      expect(
        screen.getByText(m.url_compose_map_title({ name: m.url_compose_family_anthropic() }))
      ).toBeDefined();
    });

    const dialog = screen.getByRole("dialog", {
      name: m.url_compose_map_title({ name: m.url_compose_family_anthropic() }),
    });
    // 出站地址不含版本段：/v1/messages/count_tokens → /anthropic/messages/count_tokens
    expect(dialog.textContent).toContain("https://x.com/anthropic/messages/count_tokens");
    expect(dialog.textContent).not.toContain("https://x.com/anthropic/v1/");
  });

  it("仅勾选去除版本号（前后缀全空）家族保留，取消后家族删除", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      value: { openai: { prefix: "", suffix: "", strip_version: true } },
    });
    // 勾选计入已配置摘要
    expect(screen.getByText(m.url_compose_summary_configured({ count: 1 }))).toBeDefined();

    await expand(user);
    const checkbox = screen.getByRole("checkbox", {
      name: m.url_compose_strip_version_title(),
    });
    expect(checkbox.getAttribute("data-state")).toBe("checked");

    // 取消勾选：前后缀全空且未勾选 → 家族键删除
    await user.click(checkbox);
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as {
        openai?: Record<string, unknown>;
      };
      expect(lastCall.openai).toBeUndefined();
    });
  });
  // ══════════ MY-STRIP-VERSION PATCH 9 (test) END ══════════
});
