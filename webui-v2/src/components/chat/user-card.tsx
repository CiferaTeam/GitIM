import { useEffect, useRef } from "react";
import { MessageSquare } from "lucide-react";
import { useAgentStore } from "../../hooks/use-agent-store";

interface UserCardProps {
  handler: string;
  position: { x: number; y: number };
  onClose: () => void;
  onStartDm: (handler: string) => void;
}

export function UserCard({ handler, position, onClose, onStartDm }: UserCardProps) {
  const cardRef = useRef<HTMLDivElement>(null);
  const agents = useAgentStore((s) => s.agents);
  const isAgent = agents.some((a) => a.name === handler || a.id === handler);

  // Close on click outside or Escape
  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (cardRef.current && !cardRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [onClose]);

  // Clamp position to stay within viewport
  const style: React.CSSProperties = {
    position: "fixed",
    left: Math.min(position.x, window.innerWidth - 220),
    top: Math.min(position.y + 8, window.innerHeight - 160),
    zIndex: 50,
  };

  const initial = handler.charAt(0).toUpperCase();

  return (
    <div
      ref={cardRef}
      style={style}
      className="w-52 bg-background border border-border/80 rounded-md shadow-lg p-3 flex flex-col gap-2"
    >
      {/* Avatar + handler */}
      <div className="flex items-center gap-2.5">
        <div className="h-8 w-8 rounded-full bg-primary/20 text-primary flex items-center justify-center text-sm font-semibold shrink-0">
          {initial}
        </div>
        <div className="min-w-0">
          <div className="text-sm font-medium text-foreground truncate">
            @{handler}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {isAgent ? "Agent" : "User"}
          </div>
        </div>
      </div>

      {/* Actions */}
      <button
        onClick={() => {
          onStartDm(handler);
          onClose();
        }}
        className="flex items-center gap-1.5 w-full px-2 py-1.5 text-xs rounded-md text-foreground/80 hover:bg-muted hover:text-foreground transition-colors"
      >
        <MessageSquare className="h-3.5 w-3.5" />
        <span>Send Message</span>
      </button>
    </div>
  );
}
