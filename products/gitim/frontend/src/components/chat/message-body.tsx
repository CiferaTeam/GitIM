import { useMemo, useState, useCallback } from "react";
import { Copy, Check } from "lucide-react";
import { parseMessageBody, type Fragment } from "../../lib/message-parser";

export interface MessageBodyProps {
  body: string;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

function InlineCode({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      navigator.clipboard.writeText(code).catch(() => {});
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    },
    [code]
  );

  return (
    <code
      onClick={handleClick}
      title="Click to copy"
      className={`px-1.5 py-0.5 rounded text-[13px] font-mono cursor-pointer transition-colors duration-150 ${
        copied
          ? "bg-primary/20 text-primary"
          : "bg-muted text-foreground/90 hover:bg-muted/80"
      }`}
    >
      {code}
    </code>
  );
}

function CodeBlock({ language, code }: { language?: string; code: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      navigator.clipboard.writeText(code).catch(() => {});
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    },
    [code]
  );

  return (
    <span className="block my-1.5">
      {language && (
        <span className="block text-[11px] text-muted-foreground font-mono mb-0.5">
          {language}
        </span>
      )}
      <div className="relative group">
        <button
          onClick={handleCopy}
          className="absolute right-2 top-2 hidden group-hover:flex items-center gap-1 px-1.5 py-0.5 text-xs rounded bg-background/80 border border-border/60 text-muted-foreground hover:text-foreground transition-colors"
        >
          {copied ? (
            <>
              <Check className="h-3 w-3 text-primary" />
              <span>Copied</span>
            </>
          ) : (
            <>
              <Copy className="h-3 w-3" />
              <span>Copy</span>
            </>
          )}
        </button>
        <pre className="bg-muted rounded-md p-3 overflow-x-auto text-[13px] font-mono text-foreground/90 block">
          <code>{code}</code>
        </pre>
      </div>
    </span>
  );
}

interface FragmentRendererProps {
  fragment: Fragment;
  index: number;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

function FragmentRenderer({
  fragment,
  index,
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: FragmentRendererProps) {
  switch (fragment.type) {
    case "text":
      return <span key={index}>{fragment.content}</span>;

    case "mention":
      return (
        <span
          key={index}
          className="text-primary font-medium cursor-pointer hover:underline"
          onClick={(e) => {
            e.stopPropagation();
            onMentionClick?.(fragment.handler, e);
          }}
        >
          @{fragment.handler}
        </span>
      );

    case "channel-link":
      return (
        <span
          key={index}
          className="text-primary cursor-pointer hover:underline"
          onClick={(e) => {
            e.stopPropagation();
            onChannelClick?.(fragment.channel);
          }}
        >
          #{fragment.channel}
        </span>
      );

    case "message-link":
      return (
        <span
          key={index}
          className="text-primary cursor-pointer hover:underline"
          onClick={(e) => {
            e.stopPropagation();
            onMessageLinkClick?.(fragment.channel, fragment.line);
          }}
        >
          #{fragment.channel}:L{String(fragment.line).padStart(6, "0")}
        </span>
      );

    case "user-profile":
      return (
        <span
          key={index}
          className="text-primary/70 cursor-pointer hover:underline"
          onClick={(e) => {
            e.stopPropagation();
            onUserProfileClick?.(fragment.handler, e);
          }}
        >
          ~{fragment.handler}
        </span>
      );

    case "external-link": {
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
          onClick={(e) => e.stopPropagation()}
        >
          {displayText}
        </a>
      );
    }

    case "inline-code":
      return <InlineCode key={index} code={fragment.code} />;

    case "code-block":
      return (
        <CodeBlock key={index} language={fragment.language} code={fragment.code} />
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

export function MessageBody({
  body,
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: MessageBodyProps) {
  // Agents that drive `gitim send` via shell-quoted argv often emit literal
  // backslash-n sequences in the body because bash double-quotes don't process
  // escapes. Normalize to real newlines at render time so the UI looks right
  // even when the stored body wasn't split into continuation lines.
  const fragments = useMemo(
    () => parseMessageBody(body.replace(/\\n/g, "\n")),
    [body],
  );
  return (
    <span className="whitespace-pre-wrap">
      {fragments.map((fragment, index) => (
        <FragmentRenderer
          key={index}
          fragment={fragment}
          index={index}
          onMentionClick={onMentionClick}
          onChannelClick={onChannelClick}
          onMessageLinkClick={onMessageLinkClick}
          onUserProfileClick={onUserProfileClick}
        />
      ))}
    </span>
  );
}
