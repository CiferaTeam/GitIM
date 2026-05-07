// Detect pings a fixed cheap model in the runtime (claude-haiku-4-5 / gpt-5.4-mini),
// not the user's selected model — so a green check verifies CLI availability, not model availability.
export type ProviderId = "claude" | "codex" | "opencode" | "pi" | "hermes";

export type PreflightErrorKind = "not_installed" | "timeout" | "other";

/**
 * Mirrors the `PreflightResult` struct emitted by gitim-runtime's preflight check.
 * Field names stay snake_case — this is the on-the-wire contract, not a style choice.
 */
export interface PreflightResult {
  available: boolean;
  provider: string;
  version: string | null;
  model_used: string | null;
  duration_ms: number;
  output_preview: string | null;
  error: string | null;
  error_kind: PreflightErrorKind | null;
}

export interface ProviderModel {
  id: string;
  label: string;
}

export interface ProviderInfo {
  label: string;
  models: ProviderModel[];
  /**
   * If true, Model selection is optional — the provider picks its own default
   * (e.g. opencode uses the user's `opencode auth login` default). Empty model
   * id is sent as undefined to the runtime.
   */
  modelOptional?: boolean;
}

export const PROVIDERS: Record<ProviderId, ProviderInfo> = {
  claude: {
    label: "Claude",
    models: [
      { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
      { id: "claude-opus-4-7", label: "Claude Opus 4.7" },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
    ],
  },
  codex: {
    label: "Codex",
    models: [
      { id: "gpt-5.5", label: "GPT-5.5" },
      { id: "gpt-5.4", label: "GPT-5.4" },
      { id: "gpt-5.3-codex", label: "GPT-5.3 Codex" },
    ],
  },
  opencode: {
    label: "OpenCode",
    models: [],
    modelOptional: true,
  },
  pi: {
    label: "Pi",
    models: [],
    modelOptional: true,
  },
  hermes: {
    label: "Hermes",
    models: [],
    modelOptional: true,
  },
};

export const PROVIDER_IDS: ProviderId[] = ["claude", "codex", "opencode", "pi", "hermes"];
