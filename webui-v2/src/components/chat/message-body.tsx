import { useMemo } from "react";
import { parseMessageBody, type Fragment } from "../../lib/message-parser";

interface MessageBodyProps {
  body: string;
}

function renderFragment(fragment: Fragment, index: number): React.ReactNode {
  switch (fragment.type) {
    case "text":
      return <span key={index}>{fragment.content}</span>;

    case "mention":
      return (
        <span key={index} className="text-primary font-medium">
          @{fragment.handler}
        </span>
      );

    case "channel-link":
      return (
        <span key={index} className="text-primary cursor-pointer hover:underline">
          #{fragment.channel}
        </span>
      );

    case "message-link":
      return (
        <span key={index} className="text-primary cursor-pointer hover:underline">
          #{fragment.channel}:L{String(fragment.line).padStart(6, "0")}
        </span>
      );

    case "user-profile":
      return (
        <span key={index} className="text-primary/70 cursor-pointer hover:underline">
          ~{fragment.handler}
        </span>
      );

    case "external-link": {
      // Only allow http/https
      const isAllowed =
        fragment.url.startsWith("http://") || fragment.url.startsWith("https://");
      if (!isAllowed) {
        return <span key={index}>{fragment.url}</span>;
      }
      const displayText = fragment.title
        ? fragment.title
        : fragment.url.length > 50
          ? fragment.url.slice(0, 50) + "…"
          : fragment.url;
      return (
        <a
          key={index}
          href={fragment.url}
          title={fragment.url}
          target="_blank"
          rel="noopener noreferrer"
          className="text-primary underline decoration-primary/30 hover:decoration-primary/60 transition-colors"
        >
          {displayText}
        </a>
      );
    }

    case "inline-code":
      return (
        <code
          key={index}
          className="bg-muted px-1.5 py-0.5 rounded text-[13px] font-mono text-foreground/90"
        >
          {fragment.code}
        </code>
      );

    case "code-block":
      return (
        <span key={index} className="block my-1.5">
          {fragment.language && (
            <span className="block text-[11px] text-muted-foreground font-mono mb-0.5">
              {fragment.language}
            </span>
          )}
          <pre className="bg-muted rounded-md p-3 overflow-x-auto text-[13px] font-mono text-foreground/90 block">
            <code>{fragment.code}</code>
          </pre>
        </span>
      );

    case "bold":
      return (
        <strong key={index} className="font-semibold">
          {fragment.content}
        </strong>
      );

    case "italic":
      return (
        <em key={index} className="italic">
          {fragment.content}
        </em>
      );

    default:
      return null;
  }
}

export function MessageBody({ body }: MessageBodyProps) {
  const fragments = useMemo(() => parseMessageBody(body), [body]);
  return <>{fragments.map((fragment, index) => renderFragment(fragment, index))}</>;
}
