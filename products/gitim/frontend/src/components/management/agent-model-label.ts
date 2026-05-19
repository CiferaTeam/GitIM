import type { Agent } from "@/lib/types";

export function agentModelLabel(agent: Agent) {
  if (agent.provider === "hermes" && agent.llmModel) {
    return agent.llmProvider
      ? `${agent.llmProvider} / ${agent.llmModel}`
      : agent.llmModel;
  }
  return (
    agent.model ??
    (agent.provider === "opencode" ||
    agent.provider === "pi" ||
    agent.provider === "hermes" ||
    agent.provider === "cursor" ||
    agent.provider === "kimi"
      ? "default"
      : "—")
  );
}
