import { dmPeerHandler } from "../../lib/dm-display-name";
import { cn } from "../../lib/utils";
import { HandlerName } from "./handler-name";

interface DmLabelProps {
  name: string;
  currentUser: string;
  className?: string;
}

function dmParticipants(name: string): [string, string] | null {
  const parts = name.split("--");
  return parts.length === 2 ? [parts[0], parts[1]] : null;
}

export function DmLabel({ name, currentUser, className }: DmLabelProps) {
  const peer = dmPeerHandler(name, currentUser);
  if (peer && peer !== currentUser) {
    return <HandlerName handler={peer} className={className} />;
  }

  const participants = dmParticipants(name);
  if (!participants) {
    return <span className={className}>{name}</span>;
  }

  const [left, right] = participants;
  if (left === currentUser && right === currentUser) {
    return (
      <span className={className}>
        <HandlerName handler={currentUser} />
        <span className="ml-1 font-normal text-text-muted">(me)</span>
      </span>
    );
  }

  if (left === right) {
    return <HandlerName handler={left} className={className} />;
  }

  return (
    <span className={cn("min-w-0", className)}>
      <HandlerName handler={left} />
      <span className="mx-1 text-text-faint">↔</span>
      <HandlerName handler={right} />
    </span>
  );
}
