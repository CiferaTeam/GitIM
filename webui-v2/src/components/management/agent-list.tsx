import { Button } from "@/components/ui/button";
import { useAgentStore } from "@/hooks/use-agent-store";
import { Plus } from "lucide-react";
import { AgentCard } from "./agent-card";

export function AgentList() {
  const agents = useAgentStore((s) => s.agents);

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-semibold">Agents</h1>
        <Button onClick={() => {}}>
          <Plus className="size-4 mr-1" />
          Add Agent
        </Button>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {agents.map((agent) => (
          <AgentCard key={agent.id} agent={agent} />
        ))}
      </div>
    </div>
  );
}
