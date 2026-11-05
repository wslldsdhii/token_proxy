// ══════════ HEADER-INJECTION PATCH G2 START ══════════
// 渠道级「会话标记 → 请求头注入」的纯函数：读取/写回渠道草稿、改名迁移、表单往返。
// 这些函数不依赖 React 与 i18n，便于单测覆盖全部分支。

import type {
  HeaderInjectionChannelConfig,
  HeaderInjectionChannelDraft,
  HeaderInjectionRuleConfig,
  HeaderInjectionSectionConfig,
} from "./types";

export const HEADER_INJECTION_VERSION = 1;
export const SESSION_RULE_ID = "session-id";
export const SESSION_RULE_SOURCE = "hook_session_marker";
export const DEFAULT_TARGET_HEADER = "X-Session-ID";
/** UI 下拉固定两项；默认项兼容性最好。 */
export const TARGET_HEADER_OPTIONS = ["X-Session-ID", "X-Opencode-Session"] as const;

function findSessionRule(
  channel: HeaderInjectionChannelConfig | undefined,
): HeaderInjectionRuleConfig | undefined {
  return channel?.rules?.find((rule) => (rule.id ?? "").trim() === SESSION_RULE_ID);
}

function normalizeTargetHeader(value: unknown): string {
  const trimmed = typeof value === "string" ? value.trim() : "";
  return trimmed || DEFAULT_TARGET_HEADER;
}

/** 配置段 → 表单态：补齐 version / channels，保留其余未知字段。 */
export function normalizeHeaderInjectionSection(
  value: HeaderInjectionSectionConfig | undefined,
): HeaderInjectionSectionConfig {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { version: HEADER_INJECTION_VERSION, channels: {} };
  }
  return {
    ...value,
    version: value.version ?? HEADER_INJECTION_VERSION,
    channels: { ...(value.channels ?? {}) },
  };
}

/** 读取某渠道的编辑器草稿（无记录 = 关闭 + 默认 Header）。 */
export function readChannelDraft(
  section: HeaderInjectionSectionConfig,
  channelId: string,
): HeaderInjectionChannelDraft {
  const channel = section.channels?.[channelId.trim()];
  return {
    enabled: channel?.enabled === true,
    fallbackEnabled: channel?.fallback_enabled === true,
    targetHeader: normalizeTargetHeader(findSessionRule(channel)?.target_header),
  };
}

export function isChannelEnabled(
  section: HeaderInjectionSectionConfig,
  channelId: string,
): boolean {
  return section.channels?.[channelId.trim()]?.enabled === true;
}

/**
 * 把编辑器草稿写回配置段：
 * - 从未有记录且关闭 → 不创建记录；
 * - 已有记录 → 只更新 canonical 规则，保留其他规则与未知字段；
 * - 关闭已有记录 → 保留记录并置 enabled=false。
 */
export function applyChannelDraft(
  section: HeaderInjectionSectionConfig,
  channelId: string,
  draft: HeaderInjectionChannelDraft,
): HeaderInjectionSectionConfig {
  const id = channelId.trim();
  if (!id) {
    return section;
  }
  const channels = { ...(section.channels ?? {}) };
  const existing = channels[id];
  if (!draft.enabled && !existing) {
    return section;
  }

  const base: HeaderInjectionChannelConfig = existing
    ? { ...existing }
    : { enabled: false, rules: [] };
  const rules = Array.isArray(base.rules) ? [...base.rules] : [];
  const index = rules.findIndex((rule) => (rule.id ?? "").trim() === SESSION_RULE_ID);
  const nextRule: HeaderInjectionRuleConfig = {
    ...(index >= 0 ? rules[index] : {}),
    id: SESSION_RULE_ID,
    enabled: true,
    source: SESSION_RULE_SOURCE,
    target_header: normalizeTargetHeader(draft.targetHeader),
  };
  if (index >= 0) {
    rules[index] = nextRule;
  } else {
    rules.push(nextRule);
  }

  const nextChannel: HeaderInjectionChannelConfig = {
    ...base,
    enabled: draft.enabled,
    rules,
  };
  if (draft.fallbackEnabled) {
    nextChannel.fallback_enabled = true;
  } else {
    delete nextChannel.fallback_enabled;
  }
  channels[id] = nextChannel;
  return {
    ...section,
    version: section.version ?? HEADER_INJECTION_VERSION,
    channels,
  };
}

/** 渠道改名时迁移记录；原 id 无记录时原样返回。 */export function renameChannel(
  section: HeaderInjectionSectionConfig,
  fromId: string,
  toId: string,
): HeaderInjectionSectionConfig {
  const from = fromId.trim();
  const to = toId.trim();
  if (!from || !to || from === to) {
    return section;
  }
  const channels = { ...(section.channels ?? {}) };
  const record = channels[from];
  if (!record) {
    return section;
  }
  delete channels[from];
  channels[to] = record;
  return { ...section, channels };
}

/** 编辑器保存：先按原名迁移记录，再写回当前渠道草稿。 */
export function applyEditorSave(
  section: HeaderInjectionSectionConfig,
  params: {
    /** 编辑既有渠道时的原 id；新建/复制时为 undefined。 */
    originalId?: string;
    channelId: string;
    draft: HeaderInjectionChannelDraft;
  },
): HeaderInjectionSectionConfig {
  const renamed =
    params.originalId && params.originalId.trim()
      ? renameChannel(section, params.originalId, params.channelId)
      : section;
  return applyChannelDraft(renamed, params.channelId, params.draft);
}

/** 表单态 → 配置段：没有任何渠道记录时省略整个域。 */
export function toHeaderInjectionPayload(
  section: HeaderInjectionSectionConfig,
): HeaderInjectionSectionConfig | undefined {
  const channels = section.channels ?? {};
  if (Object.keys(channels).length === 0) {
    return undefined;
  }
  return {
    ...section,
    version: section.version ?? HEADER_INJECTION_VERSION,
    channels,
  };
}
// ══════════ HEADER-INJECTION PATCH G2 END ══════════
