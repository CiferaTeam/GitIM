/**
 * TypeScript types mirroring the backend's hermes LLM introspection shapes.
 *
 * Field names are snake_case to match the on-the-wire JSON contract.
 * These types are consumed by client.ts (Task 12) and the add-agent dialog (Task 13).
 */

// ─── Provider types ───────────────────────────────────────────────────────────

/**
 * Whether the provider requires a user-supplied API key or is a custom
 * endpoint added manually via hermes config.yaml.
 *
 * Mirrors `introspect::ProviderKind` with `#[serde(rename_all = "snake_case")]`.
 */
export type HermesLlmProviderKind = "api_key" | "custom";

/**
 * Wire protocol the provider speaks.
 *
 * Mirrors `registry::ApiProtocol` with `#[serde(rename_all = "snake_case")]`.
 * Anthropic-protocol providers short-circuit model listing — the UI receives
 * an error message and falls back to the Custom model input.
 */
export type HermesApiProtocol = "open_ai" | "anthropic";

/**
 * A single LLM provider available in the user's hermes home.
 *
 * Mirrors `introspect::LlmProvider` (Serialize derives, no rename_all on struct).
 */
export interface HermesLlmProvider {
  id: string;
  label: string;
  kind: HermesLlmProviderKind;
  base_url?: string;
  /** Informational — UI doesn't branch on this; errors from model fetch encode the implication. */
  api_protocol?: HermesApiProtocol;
}

// ─── Model types ──────────────────────────────────────────────────────────────

/**
 * A single model entry returned by the /models fetch.
 *
 * Mirrors `models::ModelEntry`.
 */
export interface HermesLlmModel {
  id: string;
  label: string;
}

/**
 * Result of GET /hermes/llm/providers/{id}/models.
 *
 * `custom_allowed` is always `true` — the UI always shows the custom model
 * input regardless of whether live fetch succeeded.
 *
 * Mirrors `models::ModelListResult`.
 */
export interface HermesLlmModelList {
  models: HermesLlmModel[];
  custom_allowed: boolean;
  error: string | null;
  fetched_at_ms: number;
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

export const HERMES_DEFAULT_LLM_PROVIDER = "__default__";

/**
 * Returns true when the provider id represents a user-added custom endpoint
 * (i.e. defined in hermes config.yaml rather than the builtin registry).
 *
 * Custom provider ids always start with "custom:" by backend convention.
 */
export function isCustomProvider(id: string): boolean {
  return id.startsWith("custom:");
}

export function isHermesDefaultLlmProvider(id: string): boolean {
  return !id || id === HERMES_DEFAULT_LLM_PROVIDER;
}

export function getHermesLlmOverride(
  llmProvider: string,
  llmModel: string,
): { llmProvider: string; llmModel: string } | undefined {
  if (!llmProvider || isHermesDefaultLlmProvider(llmProvider)) {
    return undefined;
  }
  return { llmProvider, llmModel };
}

export function isHermesLlmSelectionIncomplete(
  provider: string,
  llmProvider: string,
  llmModel: string,
): boolean {
  if (provider !== "hermes" || isHermesDefaultLlmProvider(llmProvider)) {
    return false;
  }
  return !llmProvider || !llmModel;
}
