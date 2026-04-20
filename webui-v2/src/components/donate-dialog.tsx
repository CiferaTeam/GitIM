import { useState } from "react";
import { Copy, Check, Heart } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "./ui/dialog";

const DONATE_ADDRESS = "0x43B86678b4c24EfAbb15345565bb1E6FeFB6959e";
const CHAIN_NAME = "Arbitrum One";

export function DonateDialog() {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(DONATE_ADDRESS);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Ignore clipboard errors
    }
  }

  return (
    <Dialog>
      <DialogTrigger asChild>
        <button
          type="button"
          title="Support developer"
          className="flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
        >
          <Heart className="size-4" />
        </button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-md bg-background border-border">
        <DialogHeader>
          <DialogTitle className="text-foreground">Support GitIM</DialogTitle>
          <DialogDescription className="text-text-muted">
            GitIM is an independently developed project. If you find it useful,
            consider supporting the developer.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-2">
          <div className="rounded-lg border border-border bg-surface/50 p-4 space-y-3">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-text-muted uppercase tracking-wider">
                Network
              </span>
              <span className="text-sm font-medium text-foreground">
                {CHAIN_NAME}
              </span>
            </div>
            <div className="space-y-1.5">
              <span className="text-xs font-medium text-text-muted uppercase tracking-wider">
                Address
              </span>
              <div className="flex items-center gap-2">
                <code className="flex-1 text-xs font-mono bg-background border border-border rounded-md px-2.5 py-2 text-foreground break-all">
                  {DONATE_ADDRESS}
                </code>
                <button
                  type="button"
                  onClick={handleCopy}
                  className="shrink-0 flex items-center justify-center w-9 h-9 rounded-md border border-border bg-background hover:bg-surface-hover transition-colors"
                  title="Copy address"
                >
                  {copied ? (
                    <Check className="size-4 text-success" />
                  ) : (
                    <Copy className="size-4 text-text-muted" />
                  )}
                </button>
              </div>
            </div>
          </div>
          <p className="text-xs text-text-muted text-center">
            Only send tokens on {CHAIN_NAME}. Sending assets on other chains may
            result in permanent loss.
          </p>
        </div>
      </DialogContent>
    </Dialog>
  );
}
