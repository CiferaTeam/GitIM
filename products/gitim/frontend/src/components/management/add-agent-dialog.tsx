import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { toHandler, validateHandler } from "@/lib/client";
import {
  PROVIDER_IDS,
  PROVIDERS,
  resolveProviderModelCatalog,
  type PreflightResult,
  type ProviderId,
  type ProviderModelCatalog,
} from "@/lib/providers";
import {
  HERMES_DEFAULT_LLM_PROVIDER,
  getHermesLlmOverride,
  isHermesDefaultLlmProvider,
  isHermesLlmSelectionIncomplete,
  type HermesLlmModel,
  type HermesLlmProvider,
} from "@/lib/hermes-llm";
import { MAX_INTRODUCTION_LEN, type Agent } from "@/lib/types";
import { Loader2, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { EnvVarsEditor } from "./env-vars-editor";

export function AddAgentDialog() {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const addAgent = useAgentStore((s) => s.addAgent);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [provider, setProvider] = useState<ProviderId | "">("");
  const [model, setModel] = useState("");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [introduction, setIntroduction] = useState("");
  const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
  const [joinGeneral, setJoinGeneral] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  // Sticky preflight diagnostic: server returns `preflight_detail` on
  // provisioning failures (per Task 5/6 of the provisioning-preflight plan).
  // We surface it inline so the user can see which binary was missing or
  // what stdout the CLI produced, without a separate Detect roundtrip.
  const [submitError, setSubmitError] = useState<{
    error: string;
    preflight: PreflightResult | null;
  } | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const hermesModelFetchSeq = useRef(0);
  const providerModelFetchSeq = useRef(0);

  // Hermes-specific LLM selection state
  const [llmProvider, setLlmProvider] = useState("");
  const [llmModel, setLlmModel] = useState("");
  const [llmProviders, setLlmProviders] = useState<HermesLlmProvider[]>([]);
  const [llmProvidersLoading, setLlmProvidersLoading] = useState(false);
  const [llmModels, setLlmModels] = useState<HermesLlmModel[]>([]);
  const [llmModelsLoading, setLlmModelsLoading] = useState(false);
  const [llmModelsError, setLlmModelsError] = useState<string | null>(null);
  const [customModelInput, setCustomModelInput] = useState("");
  const [providerModelCatalog, setProviderModelCatalog] =
    useState<ProviderModelCatalog | null>(null);
  const [providerModelsLoading, setProviderModelsLoading] = useState(false);
  const [providerCustomModelInput, setProviderCustomModelInput] = useState("");

  const handler = toHandler(name.trim());
  const validationError = name.trim() ? validateHandler(name.trim()) : null;
  const providerInfo = provider ? PROVIDERS[provider as ProviderId] : null;
  const resolvedProviderModels = providerInfo
    ? resolveProviderModelCatalog(providerInfo, providerModelCatalog)
    : null;
  const availableModels = resolvedProviderModels?.models ?? [];
  // Custom model option sentinel value
  const CUSTOM_MODEL_VALUE = "__custom__";
  const PROVIDER_CUSTOM_MODEL_VALUE = "__provider_custom__";
  const selectedProviderModel =
    model === PROVIDER_CUSTOM_MODEL_VALUE ? providerCustomModelInput.trim() : model;
  // Effective model to pass to API: custom input overrides the select value
  const effectiveModel =
    llmModel === CUSTOM_MODEL_VALUE ? customModelInput : llmModel;
  const hermesLlmOverride = getHermesLlmOverride(llmProvider, effectiveModel);
  const selectedLlmProvider = llmProvider || HERMES_DEFAULT_LLM_PROVIDER;
  const hermesLlmUsesDefault = isHermesDefaultLlmProvider(selectedLlmProvider);

  // When GitIM provider switches to/from hermes, fetch/reset LLM providers
  useEffect(() => {
    if (provider === "hermes") {
      setLlmProvider(HERMES_DEFAULT_LLM_PROVIDER);
      setLlmProvidersLoading(true);
      client.listHermesLlmProviders().then((res) => {
        setLlmProviders(res.ok ? (res.data?.providers ?? []) : []);
        setLlmProvidersLoading(false);
      });
    } else {
      setLlmProvider("");
      setLlmModel("");
      setLlmProviders([]);
      setLlmProvidersLoading(false);
      setLlmModels([]);
      setLlmModelsLoading(false);
      setLlmModelsError(null);
      setCustomModelInput("");
    }
    setProviderModelCatalog(null);
    setProviderModelsLoading(false);
    setProviderCustomModelInput("");
  }, [provider]);

  useEffect(() => {
    if (!provider || provider === "hermes" || !PROVIDERS[provider].runtimeModels) {
      providerModelFetchSeq.current += 1;
      setProviderModelCatalog(null);
      setProviderModelsLoading(false);
      return;
    }

    const seq = ++providerModelFetchSeq.current;
    setProviderModelCatalog(null);
    setProviderModelsLoading(true);
    client.listProviderModels(provider).then((res) => {
      if (seq !== providerModelFetchSeq.current) return;
      setProviderModelCatalog(res.ok ? (res.data ?? null) : null);
      setProviderModelsLoading(false);
    });
  }, [provider]);

  // When llmProvider changes, fetch models for that provider
  useEffect(() => {
    if (!llmProvider || isHermesDefaultLlmProvider(llmProvider)) {
      hermesModelFetchSeq.current += 1;
      setLlmModels([]);
      setLlmModelsLoading(false);
      setLlmModelsError(null);
      setCustomModelInput("");
      return;
    }
    setLlmModel("");
    setCustomModelInput("");
    setLlmModelsLoading(true);
    setLlmModelsError(null);
    const seq = ++hermesModelFetchSeq.current;
    client.listHermesLlmModels(llmProvider).then((res) => {
      if (seq !== hermesModelFetchSeq.current) return; // stale, drop
      if (res.ok && res.data) {
        setLlmModels(res.data.models);
        setLlmModelsError(res.data.error);
      } else {
        setLlmModels([]);
        setLlmModelsError(res.error ?? "fetch failed");
      }
      setLlmModelsLoading(false);
    });
  }, [llmProvider]);

  function resetForm() {
    setName("");
    setProvider("");
    setModel("");
    setSystemPrompt("");
    setIntroduction("");
    setEnvVars([]);
    setJoinGeneral(true);
    setSubmitting(false);
    setSubmitError(null);
    setDetailsOpen(false);
    // Reset hermes LLM state
    setLlmProvider("");
    setLlmModel("");
    setLlmProviders([]);
    setLlmProvidersLoading(false);
    setLlmModels([]);
    setLlmModelsLoading(false);
    setLlmModelsError(null);
    setCustomModelInput("");
    setProviderModelCatalog(null);
    setProviderModelsLoading(false);
    setProviderCustomModelInput("");
  }

  function preflightKindLabel(kind: PreflightResult["error_kind"]): string {
    switch (kind) {
      case "not_installed":
        return "Provider not installed";
      case "timeout":
        return "Timed out";
      default:
        return "Other error";
    }
  }

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) resetForm();
  }

  const modelRequired = providerInfo
    ? !resolvedProviderModels?.supportsDefault && !providerInfo.modelOptional
    : true;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const hermesLlmIncomplete = isHermesLlmSelectionIncomplete(
      provider,
      llmProvider,
      effectiveModel,
    );
    if (
      !name.trim() ||
      validationError ||
      submitting ||
      !provider ||
      (modelRequired && !selectedProviderModel) ||
      hermesLlmIncomplete
    )
      return;
    if (!activeSlug) {
      toast.error("No workspace selected");
      return;
    }

    const envMap: Record<string, string> = {};
    for (const { key, value } of envVars) {
      if (key.trim()) envMap[key.trim()] = value;
    }

    setSubmitting(true);
    setSubmitError(null);
    setDetailsOpen(false);
    try {
      const res = await client.addAgent(
        activeSlug,
        name.trim(),
        provider,
        systemPrompt.trim(),
        provider === "hermes" ? model : selectedProviderModel,
        envMap,
        introduction.trim(),
        joinGeneral,
        provider === "hermes" ? hermesLlmOverride?.llmProvider : undefined,
        provider === "hermes" ? hermesLlmOverride?.llmModel : undefined,
      );
      if (res.ok && res.data?.agent) {
        addAgent(res.data.agent as Agent);
        resetForm();
        setOpen(false);
      } else {
        const errorMsg = res.error ?? "Failed to add agent";
        // Preflight failures get sticky inline rendering; everything else
        // falls back to a toast so transient errors don't pollute the form.
        if (res.preflight_detail) {
          setSubmitError({ error: errorMsg, preflight: res.preflight_detail });
        } else {
          setSubmitError({ error: errorMsg, preflight: null });
          toast.error(errorMsg);
        }
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <Button onClick={() => setOpen(true)}>
        <Plus className="size-4 mr-1" />
        Add Agent
      </Button>

      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add Agent</DialogTitle>
          </DialogHeader>

          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-provider">
                Provider
              </label>
              <select
                id="agent-provider"
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value as ProviderId | "");
                  setModel("");
                  setProviderCustomModelInput("");
                  setProviderModelCatalog(null);
                  setProviderModelsLoading(false);
                  // Clear any prior provisioning failure — the previous
                  // diagnostic is no longer relevant once the provider changes.
                  setSubmitError(null);
                  setDetailsOpen(false);
                }}
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <option value="">— Select provider —</option>
                {PROVIDER_IDS.map((id) => (
                  <option key={id} value={id}>
                    {PROVIDERS[id].label}
                  </option>
                ))}
              </select>
            </div>

            {submitError?.preflight && (
              <div className="space-y-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm">
                <div className="flex flex-col gap-1">
                  <p className="font-medium text-destructive">
                    {preflightKindLabel(submitError.preflight.error_kind)}
                  </p>
                  <p className="text-destructive/90">{submitError.error}</p>
                </div>
                <div className="flex flex-wrap gap-1.5 text-xs">
                  <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-muted-foreground">
                    provider: {submitError.preflight.provider}
                  </span>
                  {submitError.preflight.model_used && (
                    <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-muted-foreground">
                      model: {submitError.preflight.model_used}
                    </span>
                  )}
                  {submitError.preflight.version && (
                    <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-muted-foreground">
                      version: {submitError.preflight.version}
                    </span>
                  )}
                </div>
                {submitError.preflight.output_preview && (
                  <div className="space-y-1">
                    <button
                      type="button"
                      className="text-xs text-muted-foreground hover:underline"
                      onClick={() => setDetailsOpen((v) => !v)}
                    >
                      {detailsOpen ? "Hide details" : "Show details"}
                    </button>
                    {detailsOpen && (
                      <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/60 p-2 font-mono text-xs text-muted-foreground">
                        {submitError.preflight.output_preview}
                      </pre>
                    )}
                  </div>
                )}
              </div>
            )}

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-name">
                Name
              </label>
              <Input
                id="agent-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Code Reviewer"
                required
              />
              {handler && !validationError && (
                <p className="text-xs text-muted-foreground">
                  Handler: <code>{handler}</code>
                </p>
              )}
              {validationError && (
                <p className="text-xs text-destructive">{validationError}</p>
              )}
            </div>

            {provider === "hermes" ? (
              <div className="space-y-1.5">
                <label className="text-sm font-medium">Model</label>
                <p className="text-xs text-muted-foreground">
                  Hermes uses the default model from <code>hermes setup</code>{" "}
                  unless an override is selected below.
                </p>
              </div>
            ) : providerInfo ? (
              <div className="space-y-1.5">
                <label className="text-sm font-medium" htmlFor="agent-model">
                  Model
                </label>
                <select
                  id="agent-model"
                  value={model}
                  onChange={(e) => {
                    setModel(e.target.value);
                    if (e.target.value !== PROVIDER_CUSTOM_MODEL_VALUE) {
                      setProviderCustomModelInput("");
                    }
                  }}
                  disabled={!provider}
                  className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {resolvedProviderModels?.supportsDefault && (
                    <option value="">Use CLI default</option>
                  )}
                  {!resolvedProviderModels?.supportsDefault && (
                    <option value="">— Select model —</option>
                  )}
                  {availableModels.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.label}
                    </option>
                  ))}
                  {resolvedProviderModels?.supportsCustom && (
                    <option value={PROVIDER_CUSTOM_MODEL_VALUE}>Custom…</option>
                  )}
                </select>
                {providerModelsLoading && (
                  <div className="flex items-center gap-2 text-xs text-muted-foreground">
                    <Loader2 className="size-3 animate-spin" />
                    Loading runtime models…
                  </div>
                )}
                {providerModelCatalog?.error && (
                  <p className="text-xs text-muted-foreground">
                    Runtime models unavailable; default and custom values still work.
                  </p>
                )}
                {model === PROVIDER_CUSTOM_MODEL_VALUE && (
                  <Input
                    value={providerCustomModelInput}
                    onChange={(e) => setProviderCustomModelInput(e.target.value)}
                    placeholder={resolvedProviderModels?.customHint ?? "model id"}
                  />
                )}
              </div>
            ) : null}

            {provider === "hermes" && (
              <div className="space-y-3 rounded-md border border-input p-3">
                <p className="text-sm font-medium">Hermes LLM</p>

                <div className="space-y-1.5">
                  <label className="text-sm font-medium" htmlFor="hermes-llm-provider">
                    LLM Provider
                  </label>
                  <div className="space-y-1.5">
                    <select
                      id="hermes-llm-provider"
                      value={selectedLlmProvider}
                      onChange={(e) => setLlmProvider(e.target.value)}
                      className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                      <option value={HERMES_DEFAULT_LLM_PROVIDER}>
                        Default profile
                      </option>
                      {llmProviders.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.label}
                        </option>
                      ))}
                    </select>
                    {llmProvidersLoading && (
                      <div className="flex items-center gap-2 text-xs text-muted-foreground">
                        <Loader2 className="size-3 animate-spin" />
                        Loading providers…
                      </div>
                    )}
                    <p className="text-xs text-muted-foreground">
                      Default profile uses the model configured by{" "}
                      <code>hermes setup</code>.
                    </p>
                  </div>
                </div>

                {llmProvider && !hermesLlmUsesDefault && (
                  <div className="space-y-1.5">
                    <label className="text-sm font-medium" htmlFor="hermes-llm-model">
                      LLM Model
                    </label>
                    {llmModelsLoading ? (
                      <div className="flex items-center gap-2 text-sm text-muted-foreground">
                        <Loader2 className="size-4 animate-spin" />
                        Loading models…
                      </div>
                    ) : llmModelsError !== null || llmModel === CUSTOM_MODEL_VALUE ? (
                      <>
                        {llmModelsError !== null && llmModel !== CUSTOM_MODEL_VALUE && (
                          <p className="text-xs text-destructive">{llmModelsError}</p>
                        )}
                        <Input
                          id="hermes-llm-model"
                          value={customModelInput}
                          onChange={(e) => setCustomModelInput(e.target.value)}
                          placeholder="e.g. gpt-4o or custom-model-id"
                        />
                        {llmModel === CUSTOM_MODEL_VALUE && (
                          <button
                            type="button"
                            className="text-xs text-muted-foreground hover:underline"
                            onClick={() => {
                              setLlmModel("");
                              setCustomModelInput("");
                            }}
                          >
                            ← Back to model list
                          </button>
                        )}
                      </>
                    ) : (
                      <div className="flex items-start gap-2">
                        <select
                          id="hermes-llm-model"
                          value={llmModel}
                          onChange={(e) => setLlmModel(e.target.value)}
                          className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        >
                          <option value="">— Select model —</option>
                          {llmModels.map((m) => (
                            <option key={m.id} value={m.id}>
                              {m.label}
                            </option>
                          ))}
                          <option value={CUSTOM_MODEL_VALUE}>Custom…</option>
                        </select>
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-introduction">
                Introduction <span className="text-text-muted font-normal">(optional)</span>
              </label>
              <Textarea
                id="agent-introduction"
                rows={2}
                value={introduction}
                onChange={(e) =>
                  setIntroduction(e.target.value.slice(0, MAX_INTRODUCTION_LEN))
                }
                maxLength={MAX_INTRODUCTION_LEN}
                placeholder="Short blurb shown on the agent card. Not fed to the LLM."
              />
              <p className="text-xs text-text-muted text-right">
                {introduction.length} / {MAX_INTRODUCTION_LEN}
              </p>
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-prompt">
                System Prompt
              </label>
              <Textarea
                id="agent-prompt"
                rows={4}
                value={systemPrompt}
                onChange={(e) => setSystemPrompt(e.target.value)}
                className="max-h-[40vh] overflow-y-auto"
                placeholder="Describe the agent's role and behavior…"
              />
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium">
                Environment Variables
              </label>
              <EnvVarsEditor value={envVars} onChange={setEnvVars} />
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="agent-join-general"
                className="flex items-start gap-2 text-sm cursor-pointer"
              >
                <input
                  id="agent-join-general"
                  type="checkbox"
                  checked={joinGeneral}
                  onChange={(e) => setJoinGeneral(e.target.checked)}
                  className="mt-0.5 size-4 shrink-0 cursor-pointer accent-primary"
                />
                <span className="font-medium">
                  Auto-join #general channel
                </span>
              </label>
              <p className="text-xs text-text-muted pl-6">
                Uncheck if this agent should only post in specific channels.
              </p>
            </div>

            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => handleOpenChange(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={
                  !name.trim() ||
                  !!validationError ||
                  submitting ||
                  !provider ||
                  (modelRequired && !selectedProviderModel) ||
                  isHermesLlmSelectionIncomplete(
                    provider,
                    llmProvider,
                    effectiveModel,
                  )
                }
              >
                {submitting ? (
                  <>
                    <Loader2 className="size-4 mr-1 animate-spin" />
                    Adding…
                  </>
                ) : (
                  "Add agent"
                )}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
