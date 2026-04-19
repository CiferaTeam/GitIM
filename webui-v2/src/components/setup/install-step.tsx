import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { SetupShell } from "./setup-shell";

const INSTALL_CMD =
  "curl -sSf https://raw.githubusercontent.com/CiferaTeam/gitim-releases/main/install.sh | sh";
const RUN_CMD = "gitim-runtime --port 16868";
const RELEASES_URL =
  "https://github.com/CiferaTeam/gitim-releases/releases/latest";

interface InstallStepProps {
  onContinue: () => void;
}

export function InstallStep({ onContinue }: InstallStepProps) {
  return (
    <SetupShell
      step={1}
      title="Install Runtime"
      description="Download the gitim-runtime binary before you connect"
      footer={
        <>
          Currently supported:{" "}
          <code className="text-text-secondary bg-surface px-1.5 py-0.5 rounded">
            macOS arm64
          </code>
          . Other platforms: build from source.
        </>
      }
    >
      <div className="space-y-5">
        <NumberedBlock
          number={1}
          title="Install via the official script"
          cmd={INSTALL_CMD}
          note="Installs to ~/.gitim/bin. Add it to your PATH when the script tells you."
        />

        <NumberedBlock
          number={2}
          title="Start the runtime"
          cmd={RUN_CMD}
          note="Leave it running in a terminal. Default port is 16868."
        />

        <div className="text-xs text-text-muted leading-relaxed">
          Prefer a manual download? Grab a tarball from{" "}
          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary hover:underline"
          >
            gitim-releases
          </a>
          .
        </div>

        <button
          type="button"
          data-testid="install-continue"
          onClick={onContinue}
          className="w-full h-10 rounded-lg bg-primary text-primary-foreground text-sm font-semibold hover:bg-primary/90 transition-colors shadow-lg shadow-primary/20"
        >
          Runtime is running — continue
        </button>
      </div>
    </SetupShell>
  );
}

function NumberedBlock({
  number,
  title,
  cmd,
  note,
}: {
  number: number;
  title: string;
  cmd: string;
  note: string;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(cmd);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API can be blocked in some contexts; silently ignore.
    }
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <span className="w-5 h-5 rounded-full bg-primary/15 text-primary flex items-center justify-center text-[11px] font-bold shrink-0">
          {number}
        </span>
        <h3 className="text-sm font-medium text-foreground">{title}</h3>
      </div>
      <div className="relative group">
        <pre className="text-[11px] font-mono bg-surface border border-border rounded-md px-3 py-2.5 pr-10 overflow-x-auto text-text-secondary">
          <code>{cmd}</code>
        </pre>
        <button
          type="button"
          onClick={copy}
          aria-label="Copy command"
          className="absolute right-1.5 top-1.5 p-1.5 rounded text-text-muted hover:text-foreground hover:bg-background transition-colors"
        >
          {copied ? (
            <Check className="size-3.5 text-primary" />
          ) : (
            <Copy className="size-3.5" />
          )}
        </button>
      </div>
      <p className="text-[11px] text-text-muted leading-relaxed pl-7">{note}</p>
    </div>
  );
}
