import { FileText, X } from "lucide-react";

import type { AttachmentDraft } from "../../hooks/use-attachment-draft-store";

interface AttachmentDraftStripProps {
  draft: AttachmentDraft;
  error?: string;
  onRemove: (id: string) => void;
}

function displayBasename(name: string): string {
  const basename = name.split(/[\\/]/).at(-1) ?? "";
  const cleaned = Array.from(basename)
    .filter((character) => !/\p{Cc}/u.test(character))
    .join("");
  return cleaned || "attachment";
}

function formatBinarySize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MiB`;
}

function fileType(name: string, mediaType: string): string {
  const extension = name.split(".").at(-1);
  if (extension && extension !== name && extension.length <= 5) return extension.toUpperCase();
  return mediaType.split("/").at(-1)?.slice(0, 5).toUpperCase() || "FILE";
}

export function AttachmentDraftStrip({ draft, error, onRemove }: AttachmentDraftStripProps) {
  const busy = draft.status === "uploading" || draft.status === "sending";
  const operationLabel = draft.status === "uploading"
    ? "Uploading…"
    : draft.status === "sending"
      ? "Sending…"
      : null;

  return (
    <div data-attachment-draft-strip className="mb-2 min-w-0">
      <div className="flex min-w-0 flex-wrap gap-2 overflow-hidden">
        {draft.items.map((item) => {
          const name = displayBasename(item.file.name);
          return (
            <div
              key={item.id}
              className="grid w-full min-w-0 max-w-full grid-cols-[2.5rem_minmax(0,1fr)_1.75rem] items-center gap-2 rounded-lg border border-border-strong bg-surface px-1.5 py-1.5 sm:w-[13rem]"
            >
              {item.previewUrl ? (
                <img
                  src={item.previewUrl}
                  alt=""
                  className="size-10 rounded-md border border-border object-cover"
                />
              ) : (
                <div className="flex size-10 flex-col items-center justify-center rounded-md border border-border bg-background text-[9px] font-medium text-text-muted">
                  <FileText className="mb-0.5 size-3.5" />
                  {fileType(name, item.file.type)}
                </div>
              )}
              <div className="min-w-0">
                <div className="truncate text-[11px] font-medium text-foreground" title={name}>
                  {name}
                </div>
                <div className="truncate text-[10px] text-text-muted">
                  {formatBinarySize(item.file.size)}
                </div>
              </div>
              <button
                type="button"
                aria-label={`Remove ${name}`}
                title={`Remove ${name}`}
                disabled={busy}
                onClick={() => onRemove(item.id)}
                className="flex size-7 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-surface-hover hover:text-foreground disabled:cursor-not-allowed disabled:text-text-faint"
              >
                <X className="size-3.5" />
              </button>
            </div>
          );
        })}
      </div>
      {operationLabel && (
        <p className="mt-1.5 text-xs text-text-muted" role="status">
          {operationLabel}
        </p>
      )}
      {error && (
        <p className="mt-1.5 flex items-center gap-1 text-xs text-destructive" role="alert">
          <span className="inline-block size-1 rounded-full bg-destructive" />
          {error}
        </p>
      )}
    </div>
  );
}
