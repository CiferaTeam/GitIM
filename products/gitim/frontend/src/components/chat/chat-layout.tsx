import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, Hash, LayoutGrid, LogIn, Menu } from "lucide-react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChannelOperations } from "../../hooks/use-channel-operations";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { useIsMobile } from "../../hooks/use-media-query";
import { buildMentionCandidates } from "../../lib/mention-candidates";
import type { Message } from "../../lib/types";
import { workspaceIdentity } from "../../lib/workspace-key";
import { Button } from "../ui/button";
import { ChannelCardDrawer } from "../cards/channel-card-drawer";
import { MobileSidebarDrawer } from "../mobile/mobile-sidebar-drawer";
import { MobileThreadOverlay } from "../mobile/mobile-thread-overlay";
import { MobileActionSheet } from "../mobile/mobile-action-sheet";
import { ChannelActiveRuns } from "../flows/channel-active-runs";
import { ChatHeader } from "./header";
import { DmLabel } from "./dm-label";
import { InputArea } from "./input-area";
import { MessageList } from "./message-list";
import { ScrollToBottomButton } from "./scroll-to-bottom-button";
import { Sidebar } from "./sidebar";
import { ThreadPanel } from "./thread-panel";
import { UserCard } from "./user-card";
import { useScrollAtBottom } from "../../hooks/use-scroll-at-bottom";

export function ChatLayout() {
  // Scroll container ref shared between MessageList (where it attaches) and
  // useScrollAtBottom (where the jump-to-latest button reads its state).
  const messageScrollRef = useRef<HTMLDivElement | null>(null);
  const { atBottom: messagesAtBottom, scrollToBottom: scrollMessagesToBottom } =
    useScrollAtBottom(messageScrollRef);

  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const archivedChannels = useChatStore((s) => s.archivedChannels);
  const currentUser = useChatStore((s) => s.currentUser);
  const isGuest = useChatStore((s) => s.isGuest);
  const users = useChatStore((s) => s.users);
  const messages = useChatStore((s) => s.messages);
  const replyTo = useChatStore((s) => s.replyTo);
  const highlightLine = useChatStore((s) => s.highlightLine);
  const pendingScrollLine = useChatStore((s) => s.pendingScrollLine);
  const threadRoot = useChatStore((s) => s.threadRoot);
  const threadMessages = useChatStore((s) => s.threadMessages);
  const agents = useAgentStore((s) => s.agents);

  const setReplyTo = useChatStore((s) => s.setReplyTo);
  const setHighlightLine = useChatStore((s) => s.setHighlightLine);
  const setPendingScrollLine = useChatStore((s) => s.setPendingScrollLine);
  const setThreadRoot = useChatStore((s) => s.setThreadRoot);
  const navHistory = useChatStore((s) => s.navHistory);

  // All channel-business-logic (select / join / send / thread / link-jump /
  // nav back / pagination) + the viewport-anchor cache live in one hook so
  // their state can't drift apart.
  const {
    restoreAnchor,
    handleChannelSelect,
    handleJoin,
    handleStartDm,
    handleSend,
    handleShowThread,
    handleChannelClick,
    handleMessageLinkClick,
    handleNavBack,
    handleLoadOlder,
    handleViewportAnchorChange,
  } = useChannelOperations();

  const currentChannelData = currentChannel
    ? channels.find((c) => c.name === currentChannel)
    : null;
  const allMentionRecipients = useMemo(
    () =>
      currentChannelData?.kind === "channel" ? currentChannelData.members : [],
    [currentChannelData],
  );
  const mentionCandidates = useMemo(() => {
    return buildMentionCandidates({
      users,
      agents: agents.map((a) => a.id),
      includeAll: allMentionRecipients.length > 0,
    });
  }, [users, agents, allMentionRecipients]);

  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;

  // An archived channel/DM is one the user opens from an Archived section —
  // it never shows up in the active `channels` list. Read-only view: message
  // fetch already works (daemon's read handler falls back to archive/
  // automatically), but writes must be blocked.
  // A selection is "archived" iff it isn't in active channels AND either
  // matches an archived channel record OR has DM-stem shape (`--`-joined, and
  // not a known archived channel name). The shape proxy is necessary because
  // the archived DMs view is now lazy-loaded and may be null when the user
  // lands on an archived DM via a stored selection. Worst case is a
  // false-positive banner over an empty timeline if the daemon can't resolve
  // the name — recoverable on the next channel poll.
  const isArchivedView =
    !!currentChannel &&
    !currentChannelData &&
    (archivedChannels.some((c) => c.name === currentChannel) ||
      currentChannel.includes("--"));
  const showJoinBanner =
    !isArchivedView &&
    !!currentChannelData &&
    currentChannelData.kind === "channel" &&
    !currentChannelData.members.includes(currentUser);

  // Component-local transient UI state — these aren't tied to channel ops, so
  // they stay here rather than getting pulled into useChannelOperations.
  const [userCardHandler, setUserCardHandler] = useState<string | null>(null);
  const [userCardPosition, setUserCardPosition] = useState<{
    x: number;
    y: number;
  } | null>(null);
  const [cardDrawerOpen, setCardDrawerOpen] = useState(false);
  useEffect(() => {
    // Intentional: UX contract is "switching context closes transient
    // overlays". The lint rule prefers derived state for resets, but
    // cardDrawerOpen is a transient toggle with no source-of-truth to derive
    // from — re-keying ChannelCardDrawer would unmount it on every channel
    // switch instead of just closing it.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCardDrawerOpen(false);
  }, [currentChannel]);

  const handleReply = useCallback(
    (msg: Message) => {
      setReplyTo(msg);
    },
    [setReplyTo],
  );

  const handleMentionClick = useCallback(
    (handler: string, event: React.MouseEvent) => {
      setUserCardHandler(handler);
      setUserCardPosition({ x: event.clientX, y: event.clientY });
    },
    [],
  );

  const handleUserProfileClick = useCallback(
    (handler: string, event: React.MouseEvent) => {
      setUserCardHandler(handler);
      setUserCardPosition({ x: event.clientX, y: event.clientY });
    },
    [],
  );

  const handleCloseUserCard = useCallback(() => {
    setUserCardHandler(null);
    setUserCardPosition(null);
  }, []);

  const handlePendingScrollClear = useCallback(() => {
    setPendingScrollLine(null);
  }, [setPendingScrollLine]);

  const handleThreadClose = useCallback(() => {
    setThreadRoot(null);
  }, [setThreadRoot]);

  // Mobile states
  const isMobile = useIsMobile();
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [actionSheetMessage, setActionSheetMessage] = useState<Message | null>(
    null,
  );

  const handleMobileReply = useCallback(
    (msg: Message) => {
      setReplyTo(msg);
      setActionSheetMessage(null);
    },
    [setReplyTo],
  );

  const handleMobileShowThread = useCallback(
    (msg: Message) => {
      setActionSheetMessage(null);
      void handleShowThread(msg);
    },
    [handleShowThread],
  );

  const isDm = currentChannelData?.kind === "dm";
  const mobileChannelLabel = currentChannel ?? "Select a channel";

  return (
    <div className="flex h-full overflow-hidden">
      {/* Desktop sidebar */}
      <div className="hidden md:block h-full">
        <Sidebar
          onChannelSelect={handleChannelSelect}
          onStartDm={handleStartDm}
        />
      </div>

      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Mobile top bar overlay when in chat */}
        {isMobile && (
          <div className="h-[52px] border-b border-border flex items-center px-3 justify-between shrink-0 bg-card/80 backdrop-blur-md md:hidden">
            <div className="flex items-center gap-2 min-w-0 flex-1">
              <button
                onClick={() => setMobileSidebarOpen(true)}
                className="p-2 -ml-1 rounded-lg hover:bg-surface transition-colors active:scale-90"
                aria-label="Open conversations"
              >
                <Menu className="size-5 text-text-muted" />
              </button>
              {navHistory.length > 0 && (
                <button
                  onClick={handleNavBack}
                  className="p-2 rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted transition-colors active:scale-90"
                  aria-label="Back"
                >
                  <ArrowLeft className="h-4 w-4" />
                </button>
              )}
              <div className="flex items-center gap-1.5 min-w-0">
                {currentChannel && !isDm && (
                  <Hash className="size-4 text-primary shrink-0" />
                )}
                <span className="font-semibold text-sm tracking-tight truncate">
                  {currentChannel && isDm ? (
                    <DmLabel name={currentChannel} currentUser={currentUser} />
                  ) : (
                    mobileChannelLabel
                  )}
                </span>
              </div>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              {replyTo && (
                <span className="text-[11px] text-primary bg-primary/10 px-2 py-1 rounded-full">
                  Replying
                </span>
              )}
              {currentChannel && currentChannelData?.kind === "channel" && (
                <button
                  onClick={() => setCardDrawerOpen(true)}
                  className="flex items-center gap-1 px-2 py-1.5 rounded-lg text-text-muted hover:text-foreground hover:bg-surface transition-colors active:scale-90"
                  aria-label={`Open cards for ${currentChannel}`}
                >
                  <LayoutGrid className="size-4" />
                  <span className="text-xs font-medium">Cards</span>
                </button>
              )}
            </div>
          </div>
        )}

        {/* Desktop header */}
        {!isMobile && (
          <ChatHeader
            onStartDm={handleStartDm}
            onOpenCards={
              currentChannel && currentChannelData?.kind === "channel"
                ? () => setCardDrawerOpen(true)
                : undefined
            }
          >
            {navHistory.length > 0 && (
              <button
                onClick={handleNavBack}
                className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors mr-2"
                title="Back"
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                <span>Back</span>
              </button>
            )}
          </ChatHeader>
        )}

        {showJoinBanner && (
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60 bg-muted/50">
            <span className="text-xs text-muted-foreground">
              You're viewing #{currentChannel} but haven't joined
            </span>
            <Button onClick={handleJoin} variant="outline" size="xs" className="gap-1.5">
              <LogIn className="size-3" />
              Join
            </Button>
          </div>
        )}

        {isArchivedView && (
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60 bg-muted/50">
            <span className="text-xs text-muted-foreground">
              {(() => {
                // When `isArchivedView` is true the current selection isn't
                // in active channels. It's a DM iff its name is the
                // sorted-pair stem (`<min>--<max>`) and not a known archived
                // channel — the shape check works whether the view has been
                // expanded yet or not, so this banner stays correct for
                // deep-linked archived DMs.
                const isDmStem =
                  !!currentChannel &&
                  currentChannel.includes("--") &&
                  !archivedChannels.some((c) => c.name === currentChannel);
                if (isDmStem && currentChannel) {
                  const peer =
                    currentChannel
                      .split("--")
                      .find((p) => p !== currentUser) ?? currentChannel;
                  return `DM with @${peer} is archived — read only. Unarchive from the sidebar to resume the conversation.`;
                }
                return `#${currentChannel} is archived — read only. Unarchive from the sidebar to resume the conversation.`;
              })()}
            </span>
          </div>
        )}

        {currentChannel && currentChannelData?.kind === "channel" && (
          <ChannelActiveRuns channel={currentChannel} />
        )}
        <MessageList
          messages={messages}
          currentUser={currentUser}
          scopeKey={currentChannel}
          replyTo={replyTo}
          highlightLine={highlightLine}
          pendingScrollLine={pendingScrollLine}
          restoreAnchor={restoreAnchor}
          onHighlightLineChange={setHighlightLine}
          onPendingScrollClear={handlePendingScrollClear}
          onViewportAnchorChange={handleViewportAnchorChange}
          onReply={handleReply}
          onShowThread={handleShowThread}
          onMentionClick={handleMentionClick}
          onChannelClick={handleChannelClick}
          onMessageLinkClick={handleMessageLinkClick}
          onUserProfileClick={handleUserProfileClick}
          onActionSheet={isMobile ? setActionSheetMessage : undefined}
          onLoadOlder={handleLoadOlder}
          scrollRef={messageScrollRef}
        />
        {currentChannel && currentChannelData && !isArchivedView && (
          <div className="relative shrink-0">
            <ScrollToBottomButton
              visible={!messagesAtBottom}
              onClick={() => scrollMessagesToBottom()}
            />
            <InputArea
              workspaceKey={workspaceKey}
              scopeKey={currentChannel}
              replyTo={replyTo}
              onReplyToChange={setReplyTo}
              mentionCandidates={mentionCandidates}
              routing={{ kind: "channel", channel: currentChannelData }}
              messages={messages}
              currentUser={currentUser}
              disabled={isGuest}
              onSend={handleSend}
            />
          </div>
        )}
      </div>

      {/* Desktop thread panel */}
      <div className="hidden md:block">
        <ThreadPanel
          root={threadRoot}
          messages={threadMessages}
          onClose={handleThreadClose}
          onReplyInThread={handleReply}
          onMentionClick={handleMentionClick}
          onChannelClick={handleChannelClick}
          onMessageLinkClick={handleMessageLinkClick}
          onUserProfileClick={handleUserProfileClick}
        />
      </div>

      {/* Mobile overlays */}
      <MobileSidebarDrawer
        open={mobileSidebarOpen}
        onClose={() => setMobileSidebarOpen(false)}
        onChannelSelect={handleChannelSelect}
      />
      <MobileThreadOverlay
        root={threadRoot}
        messages={threadMessages}
        onClose={handleThreadClose}
        onReplyInThread={handleReply}
      />
      <MobileActionSheet
        message={actionSheetMessage}
        onClose={() => setActionSheetMessage(null)}
        onReply={handleMobileReply}
        onShowThread={handleMobileShowThread}
      />

      {userCardHandler && userCardPosition && (
        <UserCard
          handler={userCardHandler}
          position={userCardPosition}
          onClose={handleCloseUserCard}
          onStartDm={handleStartDm}
        />
      )}

      {currentChannel && currentChannelData?.kind === "channel" && (
        <ChannelCardDrawer
          channel={currentChannel}
          open={cardDrawerOpen}
          onOpenChange={setCardDrawerOpen}
        />
      )}
    </div>
  );
}
