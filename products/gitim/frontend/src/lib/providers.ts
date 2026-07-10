// Detect pings a fixed cheap model in the runtime (claude-haiku-4-5 / gpt-5.4-mini),
// not the user's selected model — so a green check verifies CLI availability, not model availability.
export type ProviderId =
  | "claude"
  | "codex"
  | "opencode"
  | "pi"
  | "hermes"
  | "cursor"
  | "kimi";

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
  /** Setup-level failure tag set by `failure_with_code` (e.g.
   *  `hermes_default_profile_no_llm`, `missing_llm_provider`,
   *  `unknown_provider`). Server omits via `skip_serializing_if`
   *  when None; absent for normal preflight failures whose top-level
   *  `error_code` is `provision_preflight_failed`. */
  failure_code?: string;
}

export interface ProviderModel {
  id: string;
  label: string;
  /** Codex model default from `codex debug models`. */
  default_effort?: string;
  /** Ordered Codex effort values advertised for this model. */
  supported_efforts?: string[];
}

export interface ProviderModelCatalog {
  provider: string;
  source: string;
  supports_default: boolean;
  supports_custom: boolean;
  custom_format_hint: string | null;
  models: ProviderModel[];
  error: string | null;
}

export interface ProviderInfo {
  label: string;
  models: ProviderModel[];
  runtimeModels?: boolean;
  supportsDefaultModel?: boolean;
  supportsCustomModel?: boolean;
  customModelHint?: string;
  /**
   * If true, Model selection is optional — the provider picks its own default
   * (e.g. opencode uses the user's `opencode auth login` default). Empty model
   * id is sent as undefined to the runtime.
   */
  modelOptional?: boolean;
}

export interface ResolvedProviderModelCatalog {
  models: ProviderModel[];
  supportsDefault: boolean;
  supportsCustom: boolean;
  customHint: string;
}

export interface ProviderModelDraft {
  model: string;
  isCustom: boolean;
  customModelInput: string;
}

export interface ResolvedProviderEffort {
  values: string[];
  defaultEffort: string | null;
}

const CLAUDE_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
const CODEX_EFFORTS = [
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
];

export function providerSupportsEffort(
  provider: ProviderId | "" | null | undefined,
): provider is "claude" | "codex" {
  return provider === "claude" || provider === "codex";
}

export function resolveProviderEffort(
  provider: ProviderId | null | undefined,
  model: string,
  models: ProviderModel[],
): ResolvedProviderEffort {
  if (provider === "claude") {
    return { values: [...CLAUDE_EFFORTS], defaultEffort: null };
  }
  if (provider !== "codex") {
    return { values: [], defaultEffort: null };
  }

  const selected = models.find((candidate) => candidate.id === model);
  const advertised = selected?.supported_efforts?.filter(Boolean) ?? [];
  return {
    values: advertised.length > 0 ? [...advertised] : [...CODEX_EFFORTS],
    defaultEffort: selected?.default_effort ?? null,
  };
}

export function normalizeProviderEffort(
  provider: ProviderId | null | undefined,
  model: string,
  models: ProviderModel[],
  effort: string,
): string {
  if (!effort) return "";
  return resolveProviderEffort(provider, model, models).values.includes(effort)
    ? effort
    : "";
}

export function effortLabel(effort: string): string {
  if (effort === "xhigh") return "XHigh";
  return effort.charAt(0).toUpperCase() + effort.slice(1);
}

export function resolveProviderModelCatalog(
  providerInfo: ProviderInfo,
  catalog: ProviderModelCatalog | null | undefined,
): ResolvedProviderModelCatalog {
  return {
    models: catalog?.models.length ? catalog.models : providerInfo.models,
    supportsDefault:
      catalog?.supports_default ?? providerInfo.supportsDefaultModel ?? false,
    supportsCustom:
      catalog?.supports_custom ?? providerInfo.supportsCustomModel ?? false,
    customHint:
      catalog?.custom_format_hint ?? providerInfo.customModelHint ?? "model id",
  };
}

export function resolveProviderModelDraft(
  currentModel: string,
  catalog: ResolvedProviderModelCatalog,
): ProviderModelDraft {
  const model = currentModel.trim();
  if (!model) {
    return { model: "", isCustom: false, customModelInput: "" };
  }
  if (catalog.models.some((m) => m.id === model)) {
    return { model, isCustom: false, customModelInput: "" };
  }
  if (catalog.supportsCustom) {
    return { model: "", isCustom: true, customModelInput: model };
  }
  return { model, isCustom: false, customModelInput: "" };
}

export const PROVIDERS: Record<ProviderId, ProviderInfo> = {
  claude: {
    label: "Claude",
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "model id accepted by claude --model",
    models: [
      { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
      { id: "claude-opus-4-8", label: "Claude Opus 4.8" },
      { id: "claude-opus-4-7", label: "Claude Opus 4.7" },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
    ],
  },
  codex: {
    label: "Codex",
    runtimeModels: true,
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "model id accepted by codex --model",
    models: [
      {
        id: "gpt-5.6-sol",
        label: "GPT-5.6-Sol",
        default_effort: "low",
        supported_efforts: [...CODEX_EFFORTS],
      },
      {
        id: "gpt-5.6-terra",
        label: "GPT-5.6-Terra",
        default_effort: "medium",
        supported_efforts: [...CODEX_EFFORTS],
      },
      {
        id: "gpt-5.6-luna",
        label: "GPT-5.6-Luna",
        default_effort: "medium",
        supported_efforts: ["low", "medium", "high", "xhigh", "max"],
      },
      {
        id: "gpt-5.5",
        label: "GPT-5.5",
        default_effort: "medium",
        supported_efforts: ["low", "medium", "high", "xhigh"],
      },
      {
        id: "gpt-5.4",
        label: "GPT-5.4",
        default_effort: "medium",
        supported_efforts: ["low", "medium", "high", "xhigh"],
      },
      {
        id: "gpt-5.3-codex",
        label: "GPT-5.3 Codex",
        supported_efforts: ["low", "medium", "high", "xhigh"],
      },
    ],
  },
  opencode: {
    label: "OpenCode",
    runtimeModels: true,
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "provider/model",
    models: [],
    modelOptional: true,
  },
  pi: {
    label: "Pi",
    runtimeModels: true,
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "provider/model or model",
    models: [],
    modelOptional: true,
  },
  hermes: {
    label: "Hermes",
    models: [],
    modelOptional: true,
  },
  cursor: {
    label: "Cursor",
    runtimeModels: true,
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "model id accepted by cursor-agent --model",
    models: [],
    modelOptional: true,
  },
  kimi: {
    label: "Kimi",
    runtimeModels: true,
    supportsDefaultModel: true,
    supportsCustomModel: true,
    customModelHint: "model id accepted by kimi set_session_model",
    models: [],
    modelOptional: true,
  },
};

export const PROVIDER_IDS: ProviderId[] = [
  "claude",
  "codex",
  "opencode",
  "pi",
  "hermes",
  "cursor",
  "kimi",
];
