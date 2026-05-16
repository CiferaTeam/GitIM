import { describe, expect, it } from "vitest";
import { PROVIDERS, resolveProviderModelCatalog } from "./providers";

describe("resolveProviderModelCatalog", () => {
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
