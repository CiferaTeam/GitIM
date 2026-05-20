import { Badge } from "@/components/ui/badge";
import type { AgentWorkState } from "@/lib/agent-runtime-state";
import type { AgentStatus } from "@/lib/types";
import { LoaderCircle } from "lucide-react";

export function relativeTime(isoString: string): string {
  const diff = Math.floor((Date.now() - new Date(isoString).getTime()) / 1000);
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} hours ago`;
  return `${Math.floor(diff / 86400)} days ago`;
}

export function statusBadge(status: AgentStatus) {
  return presenceBadge(status);
}

export function workBadge(state: AgentWorkState) {
  if (state === "working") {
    return (
      <Badge className="border border-info/30 bg-info/15 text-info hover:bg-info/20">
        <LoaderCircle className="animate-spin" />
        working
      </Badge>
    );
  }

  return (
    <Badge variant="outline" className="border-border-strong text-text-muted">
      idle
    </Badge>
  );
}

export function presenceBadge(status: AgentStatus) {
  switch (status) {
    case "running":
      return (
        <Badge className="bg-success/15 text-success border border-success/30 hover:bg-success/20">
          online
        </Badge>
      );
    case "idle":
      return (
        <Badge variant="secondary" className="text-text-muted">
          stopped
        </Badge>
      );
    case "error":
      return <Badge variant="destructive">Error</Badge>;
    case "offline":
      return <Badge variant="secondary" className="text-text-muted">stopped</Badge>;
  }
}
