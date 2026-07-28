import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
} from "react";
import {
  computeCardDraftRecipients,
  computeDraftRecipients,
} from "../../lib/recipient-preview";
import { uploadAssets, type UploadedAsset } from "../../lib/client";
import type { ApiResponse, Card, Channel, Message } from "../../lib/types";
import {
  attachmentDraftKey,
  useAttachmentDraftStore,
} from "../../hooks/use-attachment-draft-store";
import { useComposerOperationStore } from "../../hooks/use-composer-operation-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useIsMobile } from "../../hooks/use-media-query";
import { MentionPopup } from "./mention-popup";
import { HandlerName } from "./handler-name";
import { AttachmentDraftStrip } from "./attachment-draft-strip";
import { CornerDownLeft, Paperclip, SendHorizontal, UserCheck, X } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Button } from "../ui/button";
import {
  parseQuickSessionRef,
  QUICK_SESSION_DRAG_MIME,
} from "../../lib/quick-session-ref";

/**
 * Routing context for the draft-recipient preview. Required and discriminated
 * so every InputArea call site MUST declare how its messages route. A missing
 * or wrong-kind context is a compile error — not a silent "no one else".
 */
export type RecipientRouting =
  | { kind: "channel"; channel: Channel | null }
  | { kind: "card"; card: Pick<Card, "created_by" | "assignee"> | null };

interface InputAreaProps {
  /** Runtime workspace slug used by the remote asset upload endpoint. */
  workspaceSlug: string | null;
  /** Workspace identity from workspaceIdentity(mode, activeWorkspace). */
  workspaceKey: string | null;
  /** Unique key for this input's scope — used for localStorage draft keying.
   *  Channel scope: the channel display name.
   *  Card scope: "card:<channel>/<card_id>".
   *  Pass null to hide the input (e.g. when no scope is selected). */
  scopeKey: string | null;
  replyTo: Message | null;
  onReplyToChange: (msg: Message | null) => void;
  mentionCandidates: string[];
  /** How this input's messages route — drives the recipient preview.
   *  channel: group-owner + reply parent chain + mentions.
   *  card:    reporter + assignee + mentions (daemon's card_thread routing). */
  routing: RecipientRouting;
  messages?: Message[];
  currentUser?: string | null;
  disabled?: boolean;
  onSend: (body: string, pointTo: number) => Promise<ApiResponse>;
  placeholder?: string;
}

const MAX_HEIGHT = 200;
const DESKTOP_ENTER_HINT = " (Enter to send, Shift+Enter for newline)";
const ATTACHMENT_RECIPIENT_SENTINEL = "attachment";
const RUNTIME_ATTACHMENT_HELP = "Attachments require a GitIM Runtime connection.";

function draftKey(workspaceKey: string, scopeKey: string) {
  return `gitim:draft:${workspaceKey}:${scopeKey}`;
}

function clearCapturedTextStorage(
  workspaceKey: string,
  scopeKey: string,
  capturedText: string,
): void {
  const key = draftKey(workspaceKey, scopeKey);
  if (localStorage.getItem(key) === capturedText) {
    localStorage.removeItem(key);
  }
}

function resolvedPlaceholder(placeholder: string | undefined, isMobile: boolean) {
  if (placeholder) {
    return isMobile ? placeholder.replace(DESKTOP_ENTER_HINT, "") : placeholder;
  }
  return isMobile
    ? "Type a message..."
    : `Type a message...${DESKTOP_ENTER_HINT}`;
}

export function InputArea({
  workspaceSlug,
  workspaceKey,
  scopeKey,
  replyTo,
  onReplyToChange,
  mentionCandidates,
  routing,
  messages = [],
  currentUser,
  disabled,
  onSend,
  placeholder,
}: InputAreaProps) {
  const [text, setText] = useState("");
  const [sendErrors, setSendErrors] = useState<Record<string, string>>({});

  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionFilter, setMentionFilter] = useState("");
  const [mentionStart, setMentionStart] = useState(0);
  const [confirmingEmpty, setConfirmingEmpty] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const preserveSendErrorThroughNextChangeRef = useRef(false);
  const attachmentHelpId = useId();
  const textRef = useRef(text);
  textRef.current = text;
  const replyRef = useRef(replyTo);
  replyRef.current = replyTo;
  const replyChangeRef = useRef(onReplyToChange);
  replyChangeRef.current = onReplyToChange;
  const renderedScopeRef = useRef({ workspaceKey, scopeKey });
  renderedScopeRef.current = { workspaceKey, scopeKey };
  const activeScopeRef = useRef(renderedScopeRef.current);
  activeScopeRef.current = renderedScopeRef.current;
  const mountedRef = useRef(false);
  const isMobile = useIsMobile();
  const connectionMode = useConnectionStore((state) => state.mode);
  const currentAttachmentKey = workspaceKey && scopeKey
    ? attachmentDraftKey(workspaceKey, scopeKey)
    : null;
  const textBusy = useComposerOperationStore((state) =>
    currentAttachmentKey
      ? state.operations[currentAttachmentKey]?.kind === "text"
      : false);
  const completionSequence = useComposerOperationStore(
    (state) => state.completionSequence,
  );
  const lastCompletionSequenceRef = useRef(
    useComposerOperationStore.getState().completionSequence,
  );
  const attachmentDraft = useAttachmentDraftStore((state) =>
    currentAttachmentKey ? state.drafts[currentAttachmentKey] : undefined);
  const addFiles = useAttachmentDraftStore((state) => state.addFiles);
  const removeAttachment = useAttachmentDraftStore((state) => state.removeItem);
  const beginOperation = useAttachmentDraftStore((state) => state.beginOperation);
  const markUploaded = useAttachmentDraftStore((state) => state.markUploaded);
  const markSending = useAttachmentDraftStore((state) => state.markSending);
  const failOperation = useAttachmentDraftStore((state) => state.failOperation);
  const completeSuccess = useAttachmentDraftStore((state) => state.completeSuccess);
  const attachmentCapable = connectionMode === "remote" && workspaceSlug !== null;
  const hasAttachments = (attachmentDraft?.items.length ?? 0) > 0;
  const attachmentBusy = attachmentDraft?.status === "uploading" ||
    attachmentDraft?.status === "sending";
  const busy = attachmentBusy || textBusy;
  const routingBody = text.trim().length > 0
    ? text
    : hasAttachments
      ? ATTACHMENT_RECIPIENT_SENTINEL
      : "";
  const draftRecipients = useMemo(
    () =>
      routing.kind === "card"
        ? computeCardDraftRecipients({
            body: routingBody,
            card: routing.card,
            excludeSelf: currentUser,
          })
        : computeDraftRecipients({
            body: routingBody,
            channel: routing.channel,
            replyTo,
            messages,
            excludeSelf: currentUser,
          }),
    [routingBody, routing, replyTo, messages, currentUser],
  );

  useEffect(() => {
    mountedRef.current = true;
    activeScopeRef.current = renderedScopeRef.current;
    return () => {
      mountedRef.current = false;
      activeScopeRef.current = { workspaceKey: null, scopeKey: null };
    };
  }, []);

  // Restore draft when scope changes
  useEffect(() => {
    if (!workspaceKey || !scopeKey) {
      setText("");
      return;
    }
    setText(localStorage.getItem(draftKey(workspaceKey, scopeKey)) ?? "");
    setMentionOpen(false);
    setConfirmingEmpty(false);
  }, [workspaceKey, scopeKey]);

  useEffect(() => {
    const operationStore = useComposerOperationStore.getState();
    const completions = operationStore.completions.filter(
      (completion) => completion.sequence > lastCompletionSequenceRef.current,
    );
    lastCompletionSequenceRef.current = operationStore.completionSequence;
    if (!mountedRef.current || !currentAttachmentKey) return;

    for (const completion of completions) {
      if (completion.key !== currentAttachmentKey) continue;
      if (textRef.current !== completion.text) continue;
      textRef.current = "";
      setText("");
      const currentReplyLine = replyRef.current?.line_number ?? null;
      if (completion.replyLine !== null && currentReplyLine === completion.replyLine) {
        replyChangeRef.current(null);
      }
      textareaRef.current?.focus();
    }
  }, [completionSequence, currentAttachmentKey]);

  // Auto-resize textarea up to MAX_HEIGHT
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  if (disabled || !workspaceKey || !scopeKey) return null;
  // After the guard above, workspaceKey and scopeKey are non-null for the rest of render.
  const activeWorkspaceKey: string = workspaceKey;
  const activeScopeKey: string = scopeKey;
  const activeAttachmentKey = attachmentDraftKey(activeWorkspaceKey, activeScopeKey);
  const canSend = (text.trim().length > 0 || (attachmentCapable && hasAttachments)) && !busy;

  function detectMention(value: string, cursorPos: number) {
    const textBeforeCursor = value.slice(0, cursorPos);
    // Allow Unicode display names and spaces (multi-word names) while stopping
    // at protocol markers that cannot be part of a mention query.
    const match = textBeforeCursor.match(/@([^@\n\r<>]*)$/);
    if (match) {
      setMentionFilter(match[1]);
      setMentionStart(match.index!);
      setMentionOpen(true);
    } else {
      setMentionOpen(false);
    }
  }

  function handleChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const value = e.target.value;
    setText(value);
    if (preserveSendErrorThroughNextChangeRef.current) {
      preserveSendErrorThroughNextChangeRef.current = false;
    } else {
      clearSendError(activeAttachmentKey);
    }
    localStorage.setItem(draftKey(activeWorkspaceKey, activeScopeKey), value);
    const cursor = e.target.selectionStart ?? value.length;
    detectMention(value, cursor);
  }

  function clearSendError(key: string) {
    setSendErrors((current) => {
      if (current[key] === undefined) return current;
      const next = { ...current };
      delete next[key];
      return next;
    });
  }

  function setSendError(key: string, message: string) {
    setSendErrors((current) => ({ ...current, [key]: message }));
  }

  function isCurrentSendScope(requestWorkspaceKey: string, requestScopeKey: string) {
    return mountedRef.current &&
      activeScopeRef.current.workspaceKey === requestWorkspaceKey &&
      activeScopeRef.current.scopeKey === requestScopeKey;
  }

  function focusCurrentScope(requestWorkspaceKey: string, requestScopeKey: string) {
    if (isCurrentSendScope(requestWorkspaceKey, requestScopeKey)) {
      textareaRef.current?.focus();
    }
  }

  function addSelectedFiles(files: readonly File[]) {
    if (!attachmentCapable || files.length === 0) return;
    clearSendError(activeAttachmentKey);
    addFiles(activeAttachmentKey, files);
  }

  function handleAttachmentAction() {
    if (busy) return;
    if (!attachmentCapable) {
      setSendError(activeAttachmentKey, RUNTIME_ATTACHMENT_HELP);
      return;
    }
    fileInputRef.current?.click();
  }

  function failAttachmentInvariant(
    key: string,
    token: number,
    message: string,
    requestWorkspaceKey: string,
    requestScopeKey: string,
  ) {
    const failed = failOperation(key, token, message);
    if (failed) focusCurrentScope(requestWorkspaceKey, requestScopeKey);
    return failed;
  }

  // Failure/exception effects are applied only when the operation is still
  // the scope's current one: endOperation is the token-matched CAS, so a
  // stale late result renders nothing, focuses nothing, and cannot release
  // a newer operation's busy mark.
  function failTextSend(
    key: string,
    token: number,
    message: string,
    requestWorkspaceKey: string,
    requestScopeKey: string,
  ) {
    const ended = useComposerOperationStore.getState().endOperation(key, token);
    if (!ended) return;
    if (isCurrentSendScope(requestWorkspaceKey, requestScopeKey)) {
      setSendError(key, message);
    }
    focusCurrentScope(requestWorkspaceKey, requestScopeKey);
  }

  function handleFileChange(event: ChangeEvent<HTMLInputElement>) {
    const files = Array.from(event.currentTarget.files ?? []);
    addSelectedFiles(files);
    event.currentTarget.value = "";
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const files = Array.from(event.clipboardData.files ?? []);
    if (files.length === 0) return;
    if (!attachmentCapable) {
      if (event.clipboardData.getData("text/plain")) {
        preserveSendErrorThroughNextChangeRef.current = true;
        queueMicrotask(() => {
          preserveSendErrorThroughNextChangeRef.current = false;
        });
      } else {
        event.preventDefault();
      }
      setSendError(activeAttachmentKey, RUNTIME_ATTACHMENT_HELP);
      return;
    }
    event.preventDefault();
    addSelectedFiles(files);
  }

  function requestSend() {
    if (!canSend) return;
    if (draftRecipients.length === 0) {
      setMentionOpen(false);
      setConfirmingEmpty(true);
      return;
    }
    void performSend();
  }

  async function performSend() {
    const capturedWorkspaceSlug = workspaceSlug;
    const capturedWorkspaceKey = activeWorkspaceKey;
    const capturedScopeKey = activeScopeKey;
    const capturedAttachmentKey = activeAttachmentKey;
    const capturedText = text;
    const capturedHumanBody = capturedText.trim();
    const capturedReply = replyTo;
    const capturedReplyLine = capturedReply?.line_number ?? null;
    const capturedOnSend = onSend;
    const capturedDraft = useAttachmentDraftStore.getState().drafts[capturedAttachmentKey];
    const useAttachments = attachmentCapable && (capturedDraft?.items.length ?? 0) > 0;

    if (!capturedHumanBody && !useAttachments) return;
    setMentionOpen(false);
    clearSendError(capturedAttachmentKey);

    if (useAttachments && capturedWorkspaceSlug !== null) {
      const operation = beginOperation(capturedAttachmentKey);
      if (!operation) return;

      try {
        const pendingItems = operation.items.filter((item) => item.uploaded === undefined);
        let mappings: { id: string; asset: UploadedAsset }[] = [];
        if (pendingItems.length > 0) {
          const uploadResult = await uploadAssets(
            capturedWorkspaceSlug,
            pendingItems.map((item) => item.file),
          );
          const assets = uploadResult.data?.assets;
          if (!uploadResult.ok || !assets || assets.length !== pendingItems.length) {
            throw new Error(uploadResult.error ?? "Upload failed");
          }
          mappings = pendingItems.map((item, index) => ({ id: item.id, asset: assets[index] }));
        }
        if (!markUploaded(capturedAttachmentKey, operation.token, mappings)) {
          failAttachmentInvariant(
            capturedAttachmentKey,
            operation.token,
            "Uploaded attachments did not match the selected files. Try again.",
            capturedWorkspaceKey,
            capturedScopeKey,
          );
          return;
        }

        const uploadedDraft = useAttachmentDraftStore.getState().drafts[capturedAttachmentKey];
        if (
          !uploadedDraft ||
          !useComposerOperationStore
            .getState()
            .isOperationCurrent(capturedAttachmentKey, operation.token) ||
          uploadedDraft.items.some((item) => item.uploaded === undefined)
        ) {
          failAttachmentInvariant(
            capturedAttachmentKey,
            operation.token,
            "Uploaded attachment state was incomplete. Try again.",
            capturedWorkspaceKey,
            capturedScopeKey,
          );
          return;
        }
        const refs = uploadedDraft.items.map((item) => item.uploaded!.ref);
        const finalBody = [capturedHumanBody, ...refs].filter(Boolean).join("\n");
        if (!markSending(capturedAttachmentKey, operation.token)) {
          failAttachmentInvariant(
            capturedAttachmentKey,
            operation.token,
            "Attachments were not ready to send. Try again.",
            capturedWorkspaceKey,
            capturedScopeKey,
          );
          return;
        }

        const sendResult = await capturedOnSend(
          finalBody,
          capturedReply?.line_number ?? 0,
        );
        if (!sendResult.ok) {
          failAttachmentInvariant(
            capturedAttachmentKey,
            operation.token,
            sendResult.error ?? "Send failed",
            capturedWorkspaceKey,
            capturedScopeKey,
          );
          return;
        }
        // completeSuccess settles through the operation-store CAS (token
        // check + completion publish). The draft clear is identity-gated on
        // the settled items so a sync re-begin cannot lose its newer draft;
        // a stale send still bails without clearing captured text storage.
        if (
          !completeSuccess(capturedAttachmentKey, operation.token, {
            text: capturedText,
            replyLine: capturedReplyLine,
          })
        ) {
          return;
        }

        clearCapturedTextStorage(
          capturedWorkspaceKey,
          capturedScopeKey,
          capturedText,
        );
      } catch (caught) {
        failAttachmentInvariant(
          capturedAttachmentKey,
          operation.token,
          caught instanceof Error ? caught.message : "Send failed",
          capturedWorkspaceKey,
          capturedScopeKey,
        );
      }
      return;
    }

    const textOperation = useComposerOperationStore
      .getState()
      .beginOperation(capturedAttachmentKey, "text");
    if (!textOperation) return;

    try {
      const sendResult = await capturedOnSend(
        capturedHumanBody,
        capturedReply?.line_number ?? 0,
      );
      if (!sendResult.ok) {
        failTextSend(
          capturedAttachmentKey,
          textOperation.token,
          sendResult.error ?? "Send failed",
          capturedWorkspaceKey,
          capturedScopeKey,
        );
        return;
      }
      const settled = useComposerOperationStore
        .getState()
        .settleOperation(capturedAttachmentKey, textOperation.token, {
          text: capturedText,
          replyLine: capturedReplyLine,
        });
      if (!settled) return;
      clearCapturedTextStorage(
        capturedWorkspaceKey,
        capturedScopeKey,
        capturedText,
      );
    } catch (caught) {
      failTextSend(
        capturedAttachmentKey,
        textOperation.token,
        caught instanceof Error ? caught.message : "Send failed",
        capturedWorkspaceKey,
        capturedScopeKey,
      );
    }
  }

  function handleConfirmSend() {
    setConfirmingEmpty(false);
    void performSend();
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Escape" && !mentionOpen) {
      onReplyToChange(null);
      return;
    }

    if (mentionOpen) return;

    if (!isMobile && e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      requestSend();
    }
  }

  function handleMentionSelect(handle: string) {
    const ta = textareaRef.current;
    if (!ta) return;

    const cursor = ta.selectionStart ?? text.length;
    const before = text.slice(0, mentionStart);
    const after = text.slice(cursor);
    const inserted = `<@${handle}> `;
    const newText = before + inserted + after;
    setText(newText);
    localStorage.setItem(draftKey(activeWorkspaceKey, activeScopeKey), newText);
    setMentionOpen(false);

    requestAnimationFrame(() => {
      if (!ta) return;
      ta.focus();
      const newCursor = before.length + inserted.length;
      ta.setSelectionRange(newCursor, newCursor);
    });
  }

  const sendError = sendErrors[activeAttachmentKey];
  const attachmentActionLabel = attachmentCapable
    ? "Attach files"
    : "Attachments require the GitIM Runtime";

  function handleQuickSessionDrop(event: DragEvent<HTMLTextAreaElement>) {
    const encoded = event.dataTransfer.getData(QUICK_SESSION_DRAG_MIME);
    if (!encoded) return;
    event.preventDefault();
    let payload: { ref?: unknown; workspaceKey?: unknown };
    try {
      payload = JSON.parse(encoded) as {
        ref?: unknown;
        workspaceKey?: unknown;
      };
    } catch {
      return;
    }
    if (
      payload.workspaceKey !== activeWorkspaceKey ||
      typeof payload.ref !== "string" ||
      !parseQuickSessionRef(payload.ref)
    ) {
      return;
    }
    const ta = textareaRef.current;
    if (!ta) return;
    const start = ta.selectionStart ?? text.length;
    const end = ta.selectionEnd ?? start;
    const before = text.slice(0, start);
    const after = text.slice(end);
    const needsBoundarySpace = (value: string | undefined) =>
      value !== undefined && /[\p{L}\p{N}_/\\-]/u.test(value);
    const prefix = needsBoundarySpace(before.at(-1)) ? " " : "";
    const suffix =
      needsBoundarySpace(after[0]) || after.startsWith(":L") ? " " : "";
    const inserted = `${prefix}${payload.ref}${suffix}`;
    const next = before + inserted + after;
    setText(next);
    clearSendError(activeAttachmentKey);
    localStorage.setItem(draftKey(activeWorkspaceKey, activeScopeKey), next);
    requestAnimationFrame(() => {
      const cursor = start + inserted.length;
      ta.focus();
      ta.setSelectionRange(cursor, cursor);
    });
  }

  return (
    <div className="border-t border-border bg-card/60 px-4 py-3 shrink-0">
      {replyTo && (
        <div
          key={replyTo.line_number}
          className="mb-2 flex items-center gap-2 rounded-lg border border-primary/45 bg-primary/15 px-3 py-1.5 text-xs text-foreground shadow-[0_0_0_1px_rgba(96,165,250,0.12)]"
        >
          <span className="flex-1 truncate">
            <span className="font-semibold text-primary">
              Reply to <HandlerName handler={replyTo.author} />:{" "}
            </span>
            <span className="text-foreground/85">
              {replyTo.body.length > 40
                ? replyTo.body.slice(0, 40) + "..."
                : replyTo.body}
            </span>
          </span>
          <button
            onClick={() => onReplyToChange(null)}
            className="ml-1 shrink-0 rounded p-0.5 text-primary transition-colors hover:bg-primary/15 hover:text-foreground"
            aria-label="Clear reply"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}

      {attachmentDraft && (
        <AttachmentDraftStrip
          draft={attachmentDraft}
          error={sendError ?? attachmentDraft.error}
          onRemove={(id) => removeAttachment(activeAttachmentKey, id)}
        />
      )}

      <div className="relative">
        {mentionOpen && (
          <MentionPopup
            users={mentionCandidates}
            filter={mentionFilter}
            onSelect={handleMentionSelect}
            onClose={() => setMentionOpen(false)}
          />
        )}

        <textarea
          ref={textareaRef}
          rows={1}
          value={text}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          onDragOver={(event) => {
            if (event.dataTransfer.types.includes(QUICK_SESSION_DRAG_MIME)) {
              event.preventDefault();
              event.dataTransfer.dropEffect = "copy";
            }
          }}
          onDrop={handleQuickSessionDrop}
          disabled={busy}
          placeholder={resolvedPlaceholder(placeholder, isMobile)}
          enterKeyHint={isMobile ? "enter" : "send"}
          className="w-full resize-none overflow-y-auto rounded-xl border border-border bg-background py-2.5 pl-12 pr-12 text-sm transition-all placeholder:text-text-muted focus:border-ring/60 focus:outline-none focus:ring-2 focus:ring-ring/40 disabled:opacity-50 md:pr-10"
          style={{ maxHeight: `${MAX_HEIGHT}px` }}
        />

        <input
          ref={fileInputRef}
          type="file"
          multiple
          hidden
          disabled={!attachmentCapable || busy}
          onChange={handleFileChange}
        />
        <button
          type="button"
          aria-label={attachmentActionLabel}
          aria-disabled={!attachmentCapable || busy}
          aria-describedby={!attachmentCapable ? attachmentHelpId : undefined}
          title={attachmentActionLabel}
          disabled={busy}
          onClick={handleAttachmentAction}
          className="absolute bottom-2 left-2 flex size-8 items-center justify-center rounded-lg text-text-muted transition-colors hover:bg-surface-hover hover:text-foreground disabled:cursor-not-allowed disabled:text-text-faint"
        >
          <Paperclip className="size-4" />
        </button>
        {!attachmentCapable && (
          <span id={attachmentHelpId} className="sr-only">
            {RUNTIME_ATTACHMENT_HELP}
          </span>
        )}

        {isMobile ? (
          <button
            type="button"
            onClick={() => requestSend()}
            onMouseDown={(e) => e.preventDefault()}
            disabled={!canSend}
            aria-label="Send message"
            title="Send"
            className="absolute right-2 bottom-2 flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-sm transition-colors hover:bg-primary/90 active:scale-95 disabled:bg-surface disabled:text-text-faint disabled:shadow-none"
          >
            <SendHorizontal className="size-4" />
          </button>
        ) : (
          <div className="absolute right-3 top-1/2 -translate-y-1/2 flex items-center gap-1.5 pointer-events-none">
            {textBusy ? (
              <span className="text-xs text-text-muted">Sending...</span>
            ) : (
              <CornerDownLeft className="size-3.5 text-text-faint" />
            )}
          </div>
        )}
      </div>

      {(text.trim().length > 0 || hasAttachments) && (
        <div
          data-recipient-preview
          className="mt-2 flex min-h-6 flex-wrap items-center gap-1.5 text-[11px] leading-none text-text-muted"
        >
          <span className="inline-flex items-center gap-1 text-text-faint">
            <UserCheck className="size-3" />
            Routes to
          </span>
          {draftRecipients.length > 0 ? (
            draftRecipients.map((recipient) => (
              <span
                key={`${recipient}-${replyTo?.line_number ?? 0}-${draftRecipients.join("|")}`}
                data-recipient-chip
                className="route-recipient-nudge inline-flex h-6 items-center rounded-md border border-primary/45 bg-primary/15 px-2 text-[10px] font-semibold text-primary shadow-[0_0_0_1px_rgba(96,165,250,0.10)]"
              >
                <HandlerName handler={recipient} />
              </span>
            ))
          ) : (
            <span
              data-recipient-empty
              className="inline-flex h-6 items-center rounded-md border border-warning/30 bg-warning/10 px-2 font-medium text-warning"
            >
              no one else
            </span>
          )}
        </div>
      )}

      {!attachmentDraft && sendError && (
        <p
          className="mt-1.5 text-xs text-destructive flex items-center gap-1"
          role="alert"
        >
          <span className="inline-block w-1 h-1 rounded-full bg-destructive" />
          {sendError}
        </p>
      )}

      <Dialog open={confirmingEmpty} onOpenChange={setConfirmingEmpty}>
        <DialogContent
          data-testid="empty-recipients-dialog"
          className="sm:max-w-md"
          showCloseButton={false}
        >
          <DialogHeader>
            <DialogTitle>No one will receive this</DialogTitle>
            <DialogDescription>
              This message routes to no other handlers — only you would see it.
              Send anyway?
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => setConfirmingEmpty(false)}
              autoFocus
            >
              Cancel
            </Button>
            <Button
              data-testid="empty-recipients-confirm"
              onClick={handleConfirmSend}
            >
              Send anyway
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
