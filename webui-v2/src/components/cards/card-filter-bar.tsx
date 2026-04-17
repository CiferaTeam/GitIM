import { useMemo, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { LabelChipInput } from "@/components/ui/label-chip-input";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { cn } from "@/lib/utils";

export interface CardFilterState {
  channels: string[];
  labels: string[];
  assignee: string | null;
  mineOnly: boolean;
}

export const EMPTY_CARD_FILTER: CardFilterState = {
  channels: [],
  labels: [],
  assignee: null,
  mineOnly: false,
};

export interface CardFilterBarProps {
  value: CardFilterState;
  onChange: (next: CardFilterState) => void;
  labelSuggestions: string[];
}

export function CardFilterBar({
  value,
  onChange,
  labelSuggestions,
}: CardFilterBarProps) {
  const channels = useChatStore((s) => s.channels);
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);

  const channelOptions = useMemo(
    () => channels.filter((c) => c.kind === "channel").map((c) => c.name),
    [channels],
  );

  const assigneeOptions = useMemo(() => {
    const set = new Set<string>([...users, ...agents.map((a) => a.id)]);
    return [...set].sort();
  }, [users, agents]);

  const hasAny =
    value.channels.length > 0 ||
    value.labels.length > 0 ||
    !!value.assignee ||
    value.mineOnly;

  return (
    <div className="flex items-center gap-2 px-4 py-2 border-b border-border bg-[#232326] flex-wrap">
      <ChannelMulti
        options={channelOptions}
        selected={value.channels}
        onChange={(channels) => onChange({ ...value, channels })}
      />
      <div className="min-w-[220px]">
        <LabelChipInput
          value={value.labels}
          onChange={(labels) => onChange({ ...value, labels })}
          suggestions={labelSuggestions}
          allowCreate={false}
          compact
          placeholder="Labels…"
        />
      </div>
      <AssigneeSelect
        options={assigneeOptions}
        selected={value.assignee}
        disabled={value.mineOnly}
        onChange={(assignee) => onChange({ ...value, assignee })}
      />
      <label className="flex items-center gap-1.5 text-xs cursor-pointer select-none">
        <input
          type="checkbox"
          checked={value.mineOnly}
          onChange={(e) => onChange({ ...value, mineOnly: e.target.checked })}
          className="accent-[#60a5fa]"
        />
        <span>My cards</span>
      </label>
      {hasAny && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => onChange(EMPTY_CARD_FILTER)}
          className="text-xs ml-auto"
        >
          Clear all
        </Button>
      )}
    </div>
  );
}

function ChannelMulti({
  options,
  selected,
  onChange,
}: {
  options: string[];
  selected: string[];
  onChange: (next: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const label =
    selected.length === 0
      ? "All channels"
      : selected.length === 1
        ? `#${selected[0]}`
        : `${selected.length} channels`;

  function toggle(name: string) {
    if (selected.includes(name)) {
      onChange(selected.filter((n) => n !== name));
    } else {
      onChange([...selected, name]);
    }
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1 text-xs">
          {label}
          <ChevronDown className="h-3 w-3" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-56 max-h-72 overflow-auto p-1">
        {options.length === 0 ? (
          <p className="text-xs text-muted-foreground px-2 py-1.5">No channels</p>
        ) : (
          options.map((opt) => (
            <button
              key={opt}
              onClick={() => toggle(opt)}
              className={cn(
                "w-full flex items-center gap-2 px-2 py-1.5 text-xs rounded hover:bg-accent hover:text-accent-foreground",
              )}
            >
              <Check
                className={cn(
                  "h-3 w-3 shrink-0",
                  selected.includes(opt) ? "opacity-100" : "opacity-0",
                )}
              />
              <span className="truncate">#{opt}</span>
            </button>
          ))
        )}
      </PopoverContent>
    </Popover>
  );
}

function AssigneeSelect({
  options,
  selected,
  disabled,
  onChange,
}: {
  options: string[];
  selected: string | null;
  disabled: boolean;
  onChange: (next: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const label = disabled
    ? "(you)"
    : selected === "__unassigned__"
      ? "Unassigned"
      : selected
        ? `@${selected}`
        : "Anyone";

  return (
    <Popover open={disabled ? false : open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          disabled={disabled}
          className="gap-1 text-xs"
        >
          {label}
          <ChevronDown className="h-3 w-3" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-48 max-h-72 overflow-auto p-1">
        <AssigneeOption
          active={selected === null}
          onSelect={() => {
            onChange(null);
            setOpen(false);
          }}
        >
          Anyone
        </AssigneeOption>
        <AssigneeOption
          active={selected === "__unassigned__"}
          onSelect={() => {
            onChange("__unassigned__");
            setOpen(false);
          }}
        >
          Unassigned
        </AssigneeOption>
        <div className="h-px bg-border my-1" />
        {options.map((opt) => (
          <AssigneeOption
            key={opt}
            active={selected === opt}
            onSelect={() => {
              onChange(opt);
              setOpen(false);
            }}
          >
            @{opt}
          </AssigneeOption>
        ))}
      </PopoverContent>
    </Popover>
  );
}

function AssigneeOption({
  children,
  active,
  onSelect,
}: {
  children: React.ReactNode;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      className="w-full flex items-center gap-2 px-2 py-1.5 text-xs rounded hover:bg-accent hover:text-accent-foreground"
    >
      <Check
        className={cn("h-3 w-3 shrink-0", active ? "opacity-100" : "opacity-0")}
      />
      <span className="truncate">{children}</span>
    </button>
  );
}
