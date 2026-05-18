import { ChevronDown } from "lucide-react";

interface ScrollToBottomButtonProps {
  visible: boolean;
  onClick: () => void;
}

/** Floating "jump to latest" button. Renders inside an InputArea-wrapping
 *  relative container; positions itself just above the input box's top edge.
 *  Hidden via opacity + pointer-events so the fade keeps the layout stable. */
export function ScrollToBottomButton({ visible, onClick }: ScrollToBottomButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label="Jump to latest messages"
      title="Jump to latest"
      tabIndex={visible ? 0 : -1}
      aria-hidden={!visible}
      className={`absolute left-1/2 -translate-x-1/2 -top-5 z-10 flex size-9 items-center justify-center rounded-full border border-border bg-card text-foreground shadow-md transition-all duration-200 hover:bg-surface-hover hover:shadow-lg active:scale-95 ${
        visible
          ? "opacity-100 translate-y-0 pointer-events-auto"
          : "opacity-0 translate-y-2 pointer-events-none"
      }`}
    >
      <ChevronDown className="size-4" />
    </button>
  );
}
