import { lazy, Suspense, type ReactNode } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router";
import { Loader2 } from "lucide-react";
import { AppShell } from "./components/layout/app-shell";
import { useAgentActivitySSE } from "./hooks/use-agent-activity";
import { DisplayNameDirectoryProvider } from "./hooks/display-name-directory-provider";
import { useConnectionStore } from "./hooks/use-connection-store";
import { useFleetSSE } from "./hooks/use-fleet-store";
import { useIsMobile } from "./hooks/use-media-query";
import { usePollLoop } from "./hooks/use-poll-loop";
import { useWorkspaceStore } from "./hooks/use-workspace-store";
import { SetupGate } from "./components/setup/setup-gate";
import { CreateWorkspaceForm } from "./components/workspace/create-workspace-form";
import { Toaster } from "sonner";

const AgentDetail = lazy(() =>
  import("./components/management/agent-detail").then((m) => ({
    default: m.AgentDetail,
  })),
);
const AgentList = lazy(() =>
  import("./components/management/agent-list").then((m) => ({
    default: m.AgentList,
  })),
);
const BoardsView = lazy(() =>
  import("./components/boards/boards-view").then((m) => ({
    default: m.BoardsView,
  })),
);
const CardDetail = lazy(() =>
  import("./components/cards/card-detail").then((m) => ({
    default: m.CardDetail,
  })),
);
const CardKanban = lazy(() =>
  import("./components/cards/card-kanban").then((m) => ({
    default: m.CardKanban,
  })),
);
const ChatLayout = lazy(() =>
  import("./components/chat/chat-layout").then((m) => ({
    default: m.ChatLayout,
  })),
);
const CronCalendar = lazy(() =>
  import("./components/crons/cron-calendar").then((m) => ({
    default: m.CronCalendar,
  })),
);
const DocsPage = lazy(() =>
  import("./components/docs/docs-page").then((m) => ({
    default: m.DocsPage,
  })),
);
const FlowsView = lazy(() =>
  import("./components/flows/flows-view").then((m) => ({
    default: m.FlowsView,
  })),
);
const RunDetail = lazy(() =>
  import("./components/flows/run-detail").then((m) => ({
    default: m.RunDetail,
  })),
);

function PageLoading() {
  return (
    <div className="flex h-full min-h-[12rem] items-center justify-center gap-2 text-sm text-text-muted">
      <Loader2 className="size-4 animate-spin" />
      Loading...
    </div>
  );
}

function PageSuspense({ children }: { children: ReactNode }) {
  return <Suspense fallback={<PageLoading />}>{children}</Suspense>;
}

function ManagementPage() {
  return (
    <PageSuspense>
      <AgentList />
    </PageSuspense>
  );
}

function ChatPage() {
  return (
    <PageSuspense>
      <ChatLayout />
    </PageSuspense>
  );
}

function FirstRunScreen() {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-6">
      <div className="w-full max-w-md">
        <div className="text-center mb-6">
          <h1 className="text-2xl font-bold tracking-tight">gitim</h1>
          <p className="text-sm text-text-muted mt-1">
            Create your first workspace to get started.
          </p>
        </div>
        <div className="rounded-2xl border border-border bg-card/90 shadow-xl shadow-[var(--color-shadow)] p-6">
          <CreateWorkspaceForm fullWidth />
        </div>
      </div>
    </div>
  );
}

function WorkspaceLoading() {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background text-sm text-text-muted gap-2">
      <Loader2 className="size-4 animate-spin" />
      Loading workspaces...
    </div>
  );
}

function WorkspaceIncomplete({ slug }: { slug: string }) {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-6">
      <div className="max-w-md text-center space-y-3">
        <p className="text-base font-semibold">Workspace incomplete</p>
        <p className="text-sm text-text-muted">
          Workspace <code className="font-mono">{slug}</code> is registered but
          not fully initialized. Try creating it again, or delete it from the
          switcher and start over.
        </p>
      </div>
    </div>
  );
}

export default function App() {
  const mode = useConnectionStore((s) => s.mode);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspacesLoading = useWorkspaceStore((s) => s.loading);
  const isMobile = useIsMobile();
  const location = useLocation();

  // Owns workspace lifecycle: init, recursive poll, per-workspace store
  // resets, SSE sync-reset path, management-route agent refresh.
  usePollLoop();

  // SSE streams for live agent activity and fleet status — scoped to active
  // remote workspace.
  useAgentActivitySSE(mode === "remote" ? activeSlug : null);
  useFleetSSE(mode === "remote" ? activeSlug : null);

  // /docs is a standalone reference — let it render regardless of workspace
  // state so setup-screen hints ("What scopes does the PAT need?") can deep-link
  // into it without getting bounced back by the gate.
  const isDocsRoute = location.pathname.startsWith("/docs");

  // Render-time gate: until we have a workspace selected, bypass the chat UI.
  let gated: ReactNode;
  if (isDocsRoute) {
    gated = (
      <PageSuspense>
        <DocsPage />
      </PageSuspense>
    );
  } else if (mode === "local" && workspaces.length === 0) {
    gated = <WorkspaceLoading />;
  } else if (workspacesLoading && workspaces.length === 0) {
    gated = <WorkspaceLoading />;
  } else if (workspaces.length === 0) {
    gated = <FirstRunScreen />;
  } else {
    const active = activeSlug
      ? workspaces.find((w) => w.slug === activeSlug)
      : null;
    if (active && !active.initialized) {
      gated = <WorkspaceIncomplete slug={active.slug} />;
    } else {
      gated = (
        <DisplayNameDirectoryProvider>
          <Routes>
            <Route element={<AppShell />}>
            <Route
              index
              element={
                <Navigate
                  to={mode === "local" || isMobile ? "/chat" : "/management"}
                  replace
                />
              }
            />
            {mode === "remote" && (
              <>
                <Route
                  path="/management"
                  element={
                    isMobile ? <Navigate to="/chat" replace /> : <ManagementPage />
                  }
                />
                <Route
                  path="/management/:agentId"
                  element={
                    isMobile ? (
                      <Navigate to="/chat" replace />
                    ) : (
                      <PageSuspense>
                        <AgentDetail />
                      </PageSuspense>
                    )
                  }
                />
              </>
            )}
            <Route
              path="/cards"
              element={
                <PageSuspense>
                  <CardKanban />
                </PageSuspense>
              }
            />
            <Route
              path="/cards/:channel/:card_id"
              element={
                <PageSuspense>
                  <CardDetail />
                </PageSuspense>
              }
            />
            <Route
              path="/boards"
              element={
                <PageSuspense>
                  <BoardsView />
                </PageSuspense>
              }
            />
            <Route path="/chat" element={<ChatPage />} />
            {mode === "remote" && (
              <Route
                path="/crons"
                element={
                  <PageSuspense>
                    <CronCalendar />
                  </PageSuspense>
                }
              />
            )}
            {mode === "remote" && (
              <Route
                path="/flows"
                element={
                  <PageSuspense>
                    <FlowsView />
                  </PageSuspense>
                }
              />
            )}
            {mode === "remote" && (
              <Route
                path="/runs/:runId"
                element={
                  <PageSuspense>
                    <RunDetail />
                  </PageSuspense>
                }
              />
            )}
            <Route
              path="/docs"
              element={
                <PageSuspense>
                  <DocsPage />
                </PageSuspense>
              }
            />
            {mode === "local" && (
              <Route path="*" element={<Navigate to="/chat" replace />} />
            )}
            </Route>
          </Routes>
        </DisplayNameDirectoryProvider>
      );
    }
  }

  return (
    <SetupGate>
      <Toaster
        position={isMobile ? "bottom-center" : "top-right"}
        mobileOffset={{
          bottom: "calc(env(safe-area-inset-bottom) + 72px)",
          left: 16,
          right: 16,
        }}
        richColors
      />
      {gated}
    </SetupGate>
  );
}
