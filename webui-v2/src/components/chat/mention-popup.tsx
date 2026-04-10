import { useEffect, useRef, useState } from "react";
import { cn } from "../../lib/utils";

interface MentionPopupProps {
  users: string[];
  filter: string;
  onSelect: (handle: string) => void;
  onClose: () => void;
}

export function MentionPopup({ users, filter, onSelect, onClose }: MentionPopupProps) {
  const filtered = users.filter((u) =>
    u.toLowerCase().includes(filter.toLowerCase())
  );

  const [activeIndex, setActiveIndex] = useState(0);

  // Reset activeIndex when filter changes
  useEffect(() => {
    setActiveIndex(0);
  }, [filter]);

  // Use refs to avoid stale closures in the keydown listener
  const filteredRef = useRef(filtered);
  const activeIndexRef = useRef(activeIndex);
  const onSelectRef = useRef(onSelect);
  const onCloseRef = useRef(onClose);

  filteredRef.current = filtered;
  activeIndexRef.current = activeIndex;
  onSelectRef.current = onSelect;
  onCloseRef.current = onClose;

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
              i === activeIndex
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
