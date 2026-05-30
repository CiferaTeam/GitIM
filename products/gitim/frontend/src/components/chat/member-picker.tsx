import { useState } from "react";
import { Input } from "../ui/input";
import { cn } from "@/lib/utils";
import { useDirectory } from "../../hooks/use-display-name-directory";
import { resolveDisplayName } from "../../lib/format-handler-display";
import { HandlerName } from "./handler-name";

interface MemberPickerProps {
  allUsers: string[];
  excludeHandlers: string[];
  value: string[];
  onChange: (selected: string[]) => void;
  placeholder?: string;
  emptyMessage?: string;
}

export function MemberPicker({
  allUsers,
  excludeHandlers,
  value,
  onChange,
  placeholder = "Search users...",
  emptyMessage = "No users match",
}: MemberPickerProps) {
  const [query, setQuery] = useState("");
  const directory = useDirectory();

  const candidates = allUsers.filter((u) => !excludeHandlers.includes(u));
  const filtered = candidates.filter((u) => {
    const q = query.toLowerCase();
    if (u.toLowerCase().includes(q)) return true;
    const name = resolveDisplayName(u, directory);
    return name ? name.toLowerCase().includes(q) : false;
  });

  function toggle(handle: string) {
    if (value.includes(handle)) {
      onChange(value.filter((h) => h !== handle));
    } else {
      onChange([...value, handle]);
    }
  }

  function remove(handle: string) {
    onChange(value.filter((h) => h !== handle));
  }

  return (
    <div className="flex flex-col gap-2">
      {/* Selected chips — hidden when empty */}
      {value.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {value.map((handle) => (
            <span
              key={handle}
              className="inline-flex items-center gap-1 rounded-full bg-accent/20 border border-border px-2 py-0.5 text-xs font-medium text-foreground"
            >
              <HandlerName handler={handle} />
              <button
                type="button"
                aria-label={`Remove ${handle}`}
                onClick={() => remove(handle)}
                className="ml-0.5 rounded-full text-muted-foreground hover:text-foreground transition-colors"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}

      {/* Search input */}
      <Input
        placeholder={placeholder}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        className="h-7 text-xs"
      />

      {/* Candidate list */}
      <div className="max-h-48 overflow-y-auto rounded-md border border-border bg-muted/30">
        {filtered.length === 0 ? (
          <p className="px-3 py-2 text-xs text-muted-foreground">{emptyMessage}</p>
        ) : (
          <ul>
            {filtered.map((handle) => {
              const checked = value.includes(handle);
              return (
                <li key={handle}>
                  <label
                    className={cn(
                      "flex cursor-pointer items-center gap-2.5 px-3 py-1.5 text-sm transition-colors",
                      "hover:bg-accent/10",
                      checked && "bg-accent/10 text-accent-foreground"
                    )}
                  >
                    {/* Native checkbox styled to match project dark theme */}
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggle(handle)}
                      className="h-3.5 w-3.5 shrink-0 cursor-pointer accent-primary"
                    />
                    <span className="text-xs text-foreground">
                      <HandlerName handler={handle} />
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
