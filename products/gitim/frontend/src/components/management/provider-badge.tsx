import { Badge } from "@/components/ui/badge";
import type { ProviderId } from "@/lib/providers";
import { PROVIDERS } from "@/lib/providers";

interface ProviderBadgeProps {
  provider: ProviderId | undefined;
}

// Palette per provider. Follows the statusBadge convention in agent-card.tsx:
// bg-{color}/15 + text-{color} + border border-{color}/30.
const PROVIDER_CLASSES: Record<ProviderId, string> = {
  claude:
    "bg-orange-500/15 text-orange-400 border border-orange-500/30 hover:bg-orange-500/20",
  codex:
    "bg-purple-500/15 text-purple-400 border border-purple-500/30 hover:bg-purple-500/20",
  opencode:
    "bg-green-500/15 text-green-400 border border-green-500/30 hover:bg-green-500/20",
  pi: "bg-blue-500/15 text-blue-400 border border-blue-500/30 hover:bg-blue-500/20",
  hermes:
    "bg-pink-500/15 text-pink-400 border border-pink-500/30 hover:bg-pink-500/20",
  cursor:
    "bg-slate-500/15 text-slate-300 border border-slate-500/30 hover:bg-slate-500/20",
  kimi: "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 hover:bg-cyan-500/20",
};

export function ProviderBadge({ provider }: ProviderBadgeProps) {
  if (!provider) {
    return <span className="text-text-muted">—</span>;
  }
  return (
    <Badge className={PROVIDER_CLASSES[provider]}>
      {PROVIDERS[provider].label}
    </Badge>
  );
}
