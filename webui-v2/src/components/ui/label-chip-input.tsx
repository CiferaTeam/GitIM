import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";
import { Badge } from "./badge";

/* eslint-disable react-refresh/only-export-components -- co-located constants + validator match project pattern (ui/badge.tsx, ui/button.tsx, ui/tabs.tsx) */

export const LABEL_CHARSET = /^[a-z0-9_-]+$/;
export const MAX_LABEL_LENGTH = 32;
export const DEFAULT_MAX_CHIPS = 10;

export interface LabelChipInputProps {
  value: string[];
  onChange: (next: string[]) => void;
  suggestions?: string[];
  allowCreate?: boolean;
  maxChips?: number;
  placeholder?: string;
  className?: string;
  /** Compact mode: smaller padding / text; used inside filter bar. */
  compact?: boolean;
}

/** Validate one label string against backend rules. Returns null if ok, else error msg. */
export function validateLabel(label: string): string | null {
  if (label.length === 0 || label.length > MAX_LABEL_LENGTH) {
    return `Length must be 1-${MAX_LABEL_LENGTH}`;
  }
  if (!LABEL_CHARSET.test(label)) {
    return "Allowed chars: a-z 0-9 - _";
  }
  return null;
}

export function LabelChipInput({
  value,
  onChange,
  suggestions = [],
  allowCreate = true,
  maxChips = DEFAULT_MAX_CHIPS,
  placeholder = "Add label…",
  className,
  compact = false,
}: LabelChipInputProps) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [focused, setFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const filtered = suggestions
    .filter((s) => !value.includes(s))
    .filter((s) => (text ? s.toLowerCase().includes(text.toLowerCase()) : true))
    .slice(0, 10);

  // Close dropdown on blur — but delay so clicks on items register first
  useEffect(() => {
    if (!focused) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!inputRef.current) return;
      const container = inputRef.current.closest("[data-chip-root]");
      if (container && !container.contains(target)) {
        setFocused(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [focused]);

  function addLabel(raw: string) {
    const label = raw.trim().toLowerCase();
    if (!label) return;
    if (value.includes(label)) {
      setText("");
      return;
    }
    if (value.length >= maxChips) {
      setError(`Max ${maxChips} labels`);
      return;
    }
    const err = validateLabel(label);
    if (err) {
      setError(err);
      return;
    }
    if (!allowCreate && !suggestions.includes(label)) {
      setError("Not in suggestions");
      return;
    }
    onChange([...value, label]);
    setText("");
    setError(null);
  }

  function removeAt(idx: number) {
    const next = value.slice();
    next.splice(idx, 1);
    onChange(next);
    setError(null);
  }

  function handleKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      // Prefer first filtered suggestion when available
      if (filtered.length > 0) {
        addLabel(filtered[0]);
      } else if (allowCreate) {
        addLabel(text);
      }
      return;
    }
    if (e.key === "Backspace" && text.length === 0 && value.length > 0) {
      removeAt(value.length - 1);
      return;
    }
    if (e.key === "Escape") {
      setText("");
      setFocused(false);
    }
  }

  return (
    <div data-chip-root className={cn("relative", className)}>
      <div
        className={cn(
          "flex flex-wrap items-center gap-1 rounded-md border border-border bg-muted/20",
          compact ? "px-2 py-1" : "px-2 py-1.5",
          error && "border-destructive",
        )}
        onClick={() => inputRef.current?.focus()}
      >
        {value.map((label, idx) => (
          <Badge
            key={label}
            variant="secondary"
            className="gap-0.5 pr-1 text-xs"
          >
            <span>{label}</span>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                removeAt(idx);
              }}
              className="rounded-full hover:bg-background/60 p-0.5"
              aria-label={`Remove ${label}`}
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        ))}
        <input
          ref={inputRef}
          type="text"
          value={text}
          placeholder={value.length === 0 ? placeholder : ""}
          onChange={(e) => {
            setText(e.target.value.toLowerCase());
            setError(null);
          }}
          onKeyDown={handleKeyDown}
          onFocus={() => setFocused(true)}
          className="flex-1 min-w-[80px] bg-transparent outline-none text-xs placeholder:text-muted-foreground/60"
        />
      </div>
      {error && <p className="mt-1 text-xs text-destructive">{error}</p>}
      {focused && filtered.length > 0 && (
        <div className="absolute z-40 mt-1 w-full max-h-60 overflow-auto rounded-md border border-border bg-popover p-1 shadow-md">
          {filtered.map((s) => (
            <button
              key={s}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                addLabel(s);
                inputRef.current?.focus();
              }}
              className="w-full text-left px-2 py-1 text-xs rounded hover:bg-accent hover:text-accent-foreground"
            >
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
