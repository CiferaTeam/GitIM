import type { ProviderId } from "@/lib/providers";

// Model and Effort are editable in the agent detail form, but the runtime
// rejects changing them while the agent runs (http.rs `agents_patch` → 409)
// and the form greys both out under `isRunning`. Model is locked whenever it's
// editable at all (`canEditModel`); Effort is Claude-only. Both apply on the
// next Start. Other fields (prompt, env, introduction) save while running.
export function runningLockedFields(
  provider: ProviderId | null | undefined,
  canEditModel: boolean,
): Array<"Model" | "Effort"> {
  const fields: Array<"Model" | "Effort"> = [];
  if (canEditModel) fields.push("Model");
  if (provider === "claude") fields.push("Effort");
  return fields;
}

// Banner text for the edit-mode running-lock notice, or null when this agent
// has no fields that running would lock (e.g. hermes: model read-only, no effort).
export function runningLockNotice(
  provider: ProviderId | null | undefined,
  canEditModel: boolean,
): string | null {
  const fields = runningLockedFields(provider, canEditModel);
  if (fields.length === 0) return null;
  return `This agent is running — ${fields.join(
    " and ",
  )} can't be changed until you Stop it. Other fields save normally.`;
}
