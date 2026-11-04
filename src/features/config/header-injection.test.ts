import { describe, expect, it } from "vitest";

import {
  DEFAULT_TARGET_HEADER,
  SESSION_RULE_ID,
  SESSION_RULE_SOURCE,
  applyChannelDraft,
  applyEditorSave,
  isChannelEnabled,
  normalizeHeaderInjectionSection,
  readChannelDraft,
  renameChannel,
  toHeaderInjectionPayload,
} from "@/features/config/header-injection";
import type { HeaderInjectionSectionConfig } from "@/features/config/types";

function emptySection(): HeaderInjectionSectionConfig {
  return { version: 1, channels: {} };
}

describe("config/header-injection", () => {
  it("normalizes a missing section to defaults", () => {
    expect(normalizeHeaderInjectionSection(undefined)).toEqual({
      version: 1,
      channels: {},
    });
    expect(normalizeHeaderInjectionSection({ channels: { trae: { enabled: true } } })).toEqual({
      version: 1,
      channels: { trae: { enabled: true } },
    });
  });

  it("reads a missing channel as disabled with the default header", () => {
    expect(readChannelDraft(emptySection(), "trae")).toEqual({
      enabled: false,
      targetHeader: DEFAULT_TARGET_HEADER,
    });
  });

  it("does not create a record when enabling is off and none exists", () => {
    const section = emptySection();
    const next = applyChannelDraft(section, "trae", {
      enabled: false,
      targetHeader: DEFAULT_TARGET_HEADER,
    });
    expect(next).toBe(section);
    expect(next.channels).toEqual({});
  });

  it("creates the canonical rule on enable", () => {
    const next = applyChannelDraft(emptySection(), "trae", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    expect(next.channels?.trae.enabled).toBe(true);
    expect(next.channels?.trae.rules).toEqual([
      {
        id: SESSION_RULE_ID,
        enabled: true,
        source: SESSION_RULE_SOURCE,
        target_header: "X-Opencode-Session",
      },
    ]);
    expect(isChannelEnabled(next, "trae")).toBe(true);
  });

  it("keeps the record and target header when disabled", () => {
    const enabled = applyChannelDraft(emptySection(), "trae", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    const disabled = applyChannelDraft(enabled, "trae", {
      enabled: false,
      targetHeader: "X-Opencode-Session",
    });
    expect(disabled.channels?.trae.enabled).toBe(false);
    expect(disabled.channels?.trae.rules?.[0].target_header).toBe("X-Opencode-Session");
    expect(isChannelEnabled(disabled, "trae")).toBe(false);
  });

  it("preserves extra rules and unknown fields", () => {
    const section: HeaderInjectionSectionConfig = {
      version: 1,
      future_domain_flag: true,
      channels: {
        trae: {
          enabled: true,
          future_channel_flag: 7,
          rules: [
            { id: SESSION_RULE_ID, enabled: true, source: SESSION_RULE_SOURCE, target_header: "X-Session-ID" },
            { id: "future-rule", enabled: true, source: "future_source", target_header: "X-Future" },
          ],
        },
      },
    };
    const next = applyChannelDraft(section, "trae", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    expect(next.future_domain_flag).toBe(true);
    expect(next.channels?.trae.future_channel_flag).toBe(7);
    expect(next.channels?.trae.rules?.map((rule) => rule.id)).toEqual([
      SESSION_RULE_ID,
      "future-rule",
    ]);
    expect(next.channels?.trae.rules?.[0].target_header).toBe("X-Opencode-Session");
  });

  it("renames a channel record and leaves others untouched", () => {
    const section = applyChannelDraft(emptySection(), "old", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    const next = renameChannel(section, "old", "new");
    expect(next.channels?.old).toBeUndefined();
    expect(next.channels?.new.rules?.[0].target_header).toBe("X-Opencode-Session");
    expect(renameChannel(next, "missing", "other")).toBe(next);
    expect(renameChannel(section, "old", " old ")).toBe(section);
  });

  it("omits the domain from payload when no channel remains", () => {
    expect(toHeaderInjectionPayload(emptySection())).toBeUndefined();
    const section = applyChannelDraft(emptySection(), "trae", {
      enabled: true,
      targetHeader: DEFAULT_TARGET_HEADER,
    });
    expect(toHeaderInjectionPayload(section)).toEqual(section);
  });
});

describe("config/header-injection editor save", () => {
  it("does not create a record for a brand-new disabled channel", () => {
    const section = emptySection();
    const next = applyEditorSave(section, {
      channelId: "brand-new",
      draft: { enabled: false, targetHeader: DEFAULT_TARGET_HEADER },
    });
    expect(next.channels).toEqual({});
  });

  it("migrates the record when the channel id changes", () => {
    const section = applyChannelDraft(emptySection(), "old", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    const next = applyEditorSave(section, {
      originalId: "old",
      channelId: "new",
      draft: { enabled: true, targetHeader: "X-Opencode-Session" },
    });
    expect(next.channels?.old).toBeUndefined();
    expect(next.channels?.new.enabled).toBe(true);
    expect(next.channels?.new.rules?.[0].target_header).toBe("X-Opencode-Session");
  });

  it("keeps a disabled record when editing an enabled channel", () => {
    const section = applyChannelDraft(emptySection(), "trae", {
      enabled: true,
      targetHeader: "X-Opencode-Session",
    });
    const next = applyEditorSave(section, {
      originalId: "trae",
      channelId: "trae",
      draft: { enabled: false, targetHeader: "X-Opencode-Session" },
    });
    expect(next.channels?.trae.enabled).toBe(false);
    expect(next.channels?.trae.rules?.[0].target_header).toBe("X-Opencode-Session");
  });
});
