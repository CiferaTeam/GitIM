import { describe, expect, it } from "vitest";
import {
  PROVIDERS,
  normalizeProviderEffort,
  resolveProviderEffort,
  resolveProviderModelCatalog,
  resolveProviderModelDraft,
} from "./providers";

describe("resolveProviderModelCatalog", () => {
  it("keeps Codex 5.6 variants at the front of the static fallback", () => {
    expect(PROVIDERS.codex.models.slice(0, 4).map((model) => model.id)).toEqual([
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.6-luna",
      "gpt-5.5",
    ]);
  });

  it("uses runtime models ahead of static fallback models", () => {
    const resolved = resolveProviderModelCatalog(PROVIDERS.codex, {
      provider: "codex",
      source: "codex_debug_models",
      supports_default: true,
      supports_custom: true,
      custom_format_hint: "codex model id",
      models: [{ id: "gpt-live", label: "GPT Live" }],
      error: null,
    });

    expect(resolved.models).toEqual([{ id: "gpt-live", label: "GPT Live" }]);
    expect(resolved.supportsDefault).toBe(true);
    expect(resolved.supportsCustom).toBe(true);
    expect(resolved.customHint).toBe("codex model id");
  });

  it("falls back to static provider metadata when runtime catalog is empty", () => {
    const resolved = resolveProviderModelCatalog(PROVIDERS.codex, {
      provider: "codex",
      source: "codex_debug_models",
      supports_default: true,
      supports_custom: true,
      custom_format_hint: null,
      models: [],
      error: "codex not found",
    });

    expect(resolved.models).toEqual(PROVIDERS.codex.models);
    expect(resolved.supportsDefault).toBe(true);
    expect(resolved.supportsCustom).toBe(true);
    expect(resolved.customHint).toBe(PROVIDERS.codex.customModelHint);
  });

  it("keeps optional provider defaults even without static models", () => {
    const resolved = resolveProviderModelCatalog(PROVIDERS.opencode, null);

    expect(resolved.models).toEqual([]);
    expect(resolved.supportsDefault).toBe(true);
    expect(resolved.supportsCustom).toBe(true);
    expect(resolved.customHint).toBe("provider/model");
  });
});

describe("resolveProviderEffort", () => {
  it("uses the selected Codex model's advertised effort levels and default", () => {
    expect(
      resolveProviderEffort("codex", "gpt-5.6-luna", PROVIDERS.codex.models),
    ).toEqual({
      values: ["low", "medium", "high", "xhigh", "max"],
      defaultEffort: "medium",
    });
  });

  it("uses live catalog metadata ahead of static Codex metadata", () => {
    expect(
      resolveProviderEffort("codex", "gpt-live", [
        {
          id: "gpt-live",
          label: "GPT Live",
          default_effort: "high",
          supported_efforts: ["medium", "high", "ultra"],
        },
      ]),
    ).toEqual({
      values: ["medium", "high", "ultra"],
      defaultEffort: "high",
    });
  });

  it("offers the current Codex vocabulary when the model is custom or default", () => {
    expect(resolveProviderEffort("codex", "future-model", []).values).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
      "ultra",
    ]);
  });

  it("keeps Claude's existing effort levels", () => {
    expect(resolveProviderEffort("claude", "claude-opus-4-8", []).values).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
    ]);
  });
});

describe("normalizeProviderEffort", () => {
  it("clears a Codex effort that the newly selected model does not advertise", () => {
    expect(
      normalizeProviderEffort(
        "codex",
        "gpt-5.6-luna",
        PROVIDERS.codex.models,
        "ultra",
      ),
    ).toBe("");
  });

  it("preserves a supported Codex effort", () => {
    expect(
      normalizeProviderEffort(
        "codex",
        "gpt-5.6-luna",
        PROVIDERS.codex.models,
        "xhigh",
      ),
    ).toBe("xhigh");
  });
});

describe("resolveProviderModelDraft", () => {
  it("selects a runtime-listed current model instead of treating it as custom", () => {
    const resolved = resolveProviderModelCatalog(PROVIDERS.opencode, {
      provider: "opencode",
      source: "opencode_models",
      supports_default: true,
      supports_custom: true,
      custom_format_hint: "provider/model",
      models: [{ id: "openai/gpt-e2e-small", label: "openai/gpt-e2e-small" }],
      error: null,
    });

    expect(resolveProviderModelDraft("openai/gpt-e2e-small", resolved)).toEqual({
      model: "openai/gpt-e2e-small",
      isCustom: false,
      customModelInput: "",
    });
  });

  it("keeps unknown current models in the custom input when custom is supported", () => {
    const resolved = resolveProviderModelCatalog(PROVIDERS.opencode, null);

    expect(resolveProviderModelDraft("vendor/future-model", resolved)).toEqual({
      model: "",
      isCustom: true,
      customModelInput: "vendor/future-model",
    });
  });
});
