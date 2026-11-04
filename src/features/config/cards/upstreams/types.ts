// ══════════ HEADER-INJECTION PATCH G10 (types) START ══════════
import type { HeaderInjectionChannelDraft, UpstreamForm } from "@/features/config/types";
// ══════════ HEADER-INJECTION PATCH G10 (types) END ══════════

export type UpstreamColumnId =
  | "id"
  | "provider"
  | "baseUrl"
  | "apiKeys"
  | "proxyUrl"
  | "priority"
  | "status";

export type UpstreamColumnDefinition = {
  id: UpstreamColumnId;
  label: () => string;
  defaultVisible: boolean;
  headerClassName?: string;
  cellClassName?: string;
};

export type ColumnVisibility = Record<UpstreamColumnId, boolean>;

// ══════════ HEADER-INJECTION PATCH G10 (editor state) START ══════════
export type UpstreamEditorState =
  | { open: false }
  | {
      open: true;
      mode: "create";
      draft: UpstreamForm;
      headerInjection: HeaderInjectionChannelDraft;
    }
  | {
      open: true;
      mode: "edit";
      index: number;
      draft: UpstreamForm;
      headerInjection: HeaderInjectionChannelDraft;
    };
// ══════════ HEADER-INJECTION PATCH G10 (editor state) END ══════════

export type DeleteDialogState = { open: false } | { open: true; index: number };
