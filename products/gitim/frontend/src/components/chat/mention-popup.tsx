import { useEffect, useRef, useState } from "react";
import { cn } from "../../lib/utils";
import { useDirectory } from "../../hooks/use-display-name-directory";
import { resolveDisplayName } from "../../lib/format-handler-display";
import { HandlerName } from "./handler-name";

interface MentionPopupProps {
  users: string[];
  filter: string;
  onSelect: (handle: string) => void;
  onClose: () => void;
}

export function MentionPopup({ users, filter, onSelect, onClose }: MentionPopupProps) {
  const directory = useDirectory();
  const f = filter.toLowerCase();
  // Match on either segment: typing the handle OR the display name finds the
  // user. Selection still inserts the bare handle (handled by onSelect).
  const filtered = users.filter((u) => {
    if (u.toLowerCase().includes(f)) return true;
    const name = resolveDisplayName(u, directory);
    return name ? name.toLowerCase().includes(f) : false;
  });

  const [activeIndex, setActiveIndex] = useState(0);
  const selectedIndex =
    filtered.length === 0 ? 0 : Math.min(activeIndex, filtered.length - 1);

  // Use refs to avoid stale closures in the keydown listener
  const filteredRef = useRef(filtered);
  const activeIndexRef = useRef(activeIndex);
  const onSelectRef = useRef(onSelect);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    filteredRef.current = filtered;
    activeIndexRef.current = selectedIndex;
    onSelectRef.current = onSelect;
    onCloseRef.current = onClose;
  }, [filtered, selectedIndex, onSelect, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const list = filteredRef.current;
      if (list.length === 0) {
        return;
      }
      const idx = activeIndexRef.current;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        e.stopPropagation();
        setActiveIndex((i) => (i + 1) % list.length);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        setActiveIndex((i) => (i - 1 + list.length) % list.length);
      } else if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        e.stopPropagation();
        if (list[idx]) {
          onSelectRef.current(list[idx]);
        }
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCloseRef.current();
      }
    }

    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, []);

  if (filtered.length === 0) return null;

  return (
    <div className="absolute bottom-full left-0 mb-1 z-50 w-56 rounded-md border bg-popover shadow-md overflow-hidden">
      <div className="max-h-48 overflow-y-auto">
        {filtered.map((user, i) => (
          <button
            key={user}
            className={cn(
              "w-full truncate text-left px-3 py-1.5 text-sm transition-colors",
              i === selectedIndex
                ? "bg-accent text-accent-foreground"
                : "hover:bg-muted"
            )}
            onMouseEnter={() => setActiveIndex(i)}
            onMouseDown={(e) => {
              // Prevent blur on the textarea
              e.preventDefault();
              onSelect(user);
            }}
          >
            <HandlerName handler={user} />
          </button>
        ))}
      </div>
    </div>
  );
}
