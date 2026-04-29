import { useEffect, useMemo, useRef, useState } from "react";
import { cn } from "../../lib/utils";

interface MentionPopupProps {
  users: string[];
  filter: string;
  onSelect: (handle: string) => void;
  onClose: () => void;
}

export function MentionPopup({ users, filter, onSelect, onClose }: MentionPopupProps) {
  const filtered = useMemo(
    () => users.filter((u) => u.toLowerCase().includes(filter.toLowerCase())),
    [users, filter],
  );

  const [activeIndex, setActiveIndex] = useState(0);
  const boundedActiveIndex =
    filtered.length > 0 ? Math.min(activeIndex, filtered.length - 1) : 0;

  // Use refs to avoid stale closures in the keydown listener
  const filteredRef = useRef(filtered);
  const activeIndexRef = useRef(activeIndex);
  const onSelectRef = useRef(onSelect);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    filteredRef.current = filtered;
    activeIndexRef.current = boundedActiveIndex;
    onSelectRef.current = onSelect;
    onCloseRef.current = onClose;
  }, [filtered, boundedActiveIndex, onSelect, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const list = filteredRef.current;
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
              "w-full text-left px-3 py-1.5 text-sm transition-colors",
              i === boundedActiveIndex
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
            @{user}
          </button>
        ))}
      </div>
    </div>
  );
}
