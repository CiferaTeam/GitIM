import { useDirectory } from "../../hooks/use-display-name-directory";
import { resolveDisplayName } from "../../lib/format-handler-display";
import { cn } from "../../lib/utils";

interface HandlerNameProps {
  handler: string;
  /** Classes for the wrapper `<span>` (typically the display-name styling). */
  className?: string;
  /** Extra classes for the muted `@handler` segment. */
  handleClassName?: string;
  /**
   * When `false`, render only the display name and drop the `@handler` segment.
   * A handler with no known display_name still falls back to bare `@handler`
   * (there's nothing else to show). Default `true`.
   */
  showHandle?: boolean;
}

/**
 * The single render point for a handler in the chat surface. Looks the handler
 * up in the display-name directory and renders `display_name @handler` (with
 * the `@handler` segment muted/monospace per DESIGN.md — handler is a technical
 * value). Unknown handler, or display_name === handler, falls back to a bare
 * `@handler`. Protocol/mention/wake-up still operate purely on the handler.
 */
export function HandlerName({
  handler,
  className,
  handleClassName,
  showHandle = true,
}: HandlerNameProps) {
  const directory = useDirectory();
  const name = resolveDisplayName(handler, directory);

  if (!name) {
    return <span className={className}>@{handler}</span>;
  }

  return (
    <span className={className}>
      {name}
      {showHandle && (
        <span
          className={cn(
            "ml-1 font-mono text-[0.85em] font-normal text-text-muted",
            handleClassName,
          )}
        >
          @{handler}
        </span>
      )}
    </span>
  );
}
