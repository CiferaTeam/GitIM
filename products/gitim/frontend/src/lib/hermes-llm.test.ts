import { describe, expect, it } from "vitest";
import {
  HERMES_DEFAULT_LLM_PROVIDER,
  getHermesLlmOverride,
  isHermesLlmSelectionIncomplete,
} from "./hermes-llm";

describe("Hermes LLM selection", () => {
  it("treats the default profile as a complete Hermes selection", () => {
    expect(
      isHermesLlmSelectionIncomplete(
        "hermes",
        HERMES_DEFAULT_LLM_PROVIDER,
        "",
      ),
    ).toBe(false);
    expect(
      getHermesLlmOverride(HERMES_DEFAULT_LLM_PROVIDER, ""),
    ).toBeUndefined();
  });

  it("requires a model when a concrete provider override is selected", () => {
    expect(isHermesLlmSelectionIncomplete("hermes", "anthropic", "")).toBe(true);
    expect(
      isHermesLlmSelectionIncomplete("hermes", "anthropic", "claude"),
    ).toBe(false);
    expect(getHermesLlmOverride("anthropic", "claude")).toEqual({
      llmProvider: "anthropic",
      llmModel: "claude",
    });
  });
});
