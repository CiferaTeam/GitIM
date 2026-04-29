import { Badge } from "@/components/ui/badge";
import type { AgentStatus } from "@/lib/types";

export function AgentStatusBadge({ status }: { status: AgentStatus }) {
  switch (status) {
    case "running":
      return (
        <Badge className="bg-success text-white hover:bg-success">
          Running
        </Badge>
      );
    case "idle":
      return (
        <Badge className="bg-warning text-white hover:bg-warning">
          Idle
        </Badge>
      );
    case "error":
      return <Badge variant="destructive">Error</Badge>;
    case "offline":
      return <Badge variant="secondary">Offline</Badge>;
  }
}
