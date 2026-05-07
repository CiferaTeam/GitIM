import { Badge } from "@/components/ui/badge";
import type { AgentStatus } from "@/lib/types";

export function relativeTime(isoString: string): string {
  const diff = Math.floor((Date.now() - new Date(isoString).getTime()) / 1000);
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} hours ago`;
  return `${Math.floor(diff / 86400)} days ago`;
}

export function statusBadge(status: AgentStatus) {
  switch (status) {
    case "running":
      return (
        <Badge className="bg-success/15 text-success border border-success/30 hover:bg-success/20">
          Running
        </Badge>
      );
    case "idle":
      return (
        <Badge className="bg-warning/15 text-warning border border-warning/30 hover:bg-warning/20">
          Idle
        </Badge>
      );
    case "error":
      return <Badge variant="destructive">Error</Badge>;
    case "offline":
      return <Badge variant="secondary" className="text-text-muted">Offline</Badge>;
  }
}
