import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import {
  ArrowLeft,
  CircleHelp,
  Users,
  MessageSquare,
  LayoutDashboard,
  ChevronRight,
  KeyRound,
} from "lucide-react";
import { DEFAULT_RUNTIME_PORT } from "@/lib/constants";

interface DocSection {
  id: string;
  title: string;
  icon: React.ReactNode;
  content: React.ReactNode;
}

function Step({ number, title, children }: { number: number; title: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-4">
      <div className="flex flex-col items-center">
        <div className="w-7 h-7 rounded-full bg-primary/15 text-primary flex items-center justify-center text-sm font-bold shrink-0">
          {number}
        </div>
        <div className="w-px flex-1 bg-border mt-2" />
      </div>
      <div className="pb-6 flex-1">
        <h3 className="text-sm font-semibold text-foreground mb-2">{title}</h3>
        <div className="text-sm text-text-secondary leading-relaxed space-y-2">{children}</div>
      </div>
    </div>
  );
}

function Tip({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-primary/20 bg-primary/5 px-4 py-3 text-sm text-text-secondary">
      <span className="font-semibold text-primary mr-1">Tip:</span>
      {children}
    </div>
  );
}

/**
 * Screenshot slot. Swap `src="pending:..."` for a real path once the image
 * is dropped into `products/gitim/frontend/public/docs-images/...`.
 */
function Screenshot({ src, caption }: { src: string; caption: string }) {
  const pending = src.startsWith("pending:");
  const target = pending ? src.slice("pending:".length) : src;
  return (
    <figure className="rounded-md border border-border bg-surface/40 overflow-hidden">
      {pending ? (
        <div className="aspect-video flex flex-col items-center justify-center gap-1.5 bg-surface/60 text-center p-6">
          <span className="text-[10px] font-semibold text-text-muted uppercase tracking-wider">
            Screenshot pending
          </span>
          <code className="font-mono text-[11px] text-text-secondary break-all max-w-full">
            {target}
          </code>
        </div>
      ) : (
        <img src={target} alt={caption} className="w-full block" loading="lazy" />
      )}
      <figcaption className="text-[11px] text-text-muted px-3 py-2 border-t border-border/60">
        {caption}
      </figcaption>
    </figure>
  );
}

export function DocsPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const initialTab = searchParams.get("tab") ?? "quickstart";
  const [activeId, setActiveId] = useState(initialTab);

  function selectTab(id: string) {
    setActiveId(id);
    // Keep URL in sync so refresh / bookmark lands on the same section.
    setSearchParams({ tab: id }, { replace: true });
  }

  const sections: DocSection[] = [
    {
      id: "quickstart",
      title: "Quick Start",
      icon: <CircleHelp className="size-4" />,
      content: (
        <div className="space-y-2">
          <p className="text-text-secondary text-sm leading-relaxed">
            gitim is a lightweight chat client that connects to a local GitIM daemon.
            This guide walks you through the essentials to get up and running in minutes.
          </p>

          <Step number={1} title="Connect to the daemon">
            <p>
              On first launch, you will see a setup screen asking for the daemon port.
              The default is <code className="font-mono text-xs bg-surface px-1.5 py-0.5 rounded border border-border">localhost:{DEFAULT_RUNTIME_PORT}</code>.
              Make sure the GitIM daemon is running locally before connecting.
            </p>
            <Tip>
              If the connection fails, check that the daemon is running and the port matches.
              You can change the port anytime from the connection settings.
            </Tip>
          </Step>

          <Step number={2} title="Create or select a workspace">
            <p>
              A workspace maps to a Git repository where all messages and data are stored.
              Use the dropdown in the top-left corner to create a new workspace or switch between existing ones.
            </p>
            <p>
              Each workspace is independent — channels, DMs, and cards do not share across workspaces.
            </p>
          </Step>

          <Step number={3} title="You're in">
            <p>
              Once connected, the top navigation shows three tabs: <strong>Agents</strong>, <strong>Chat</strong>, and <strong>Cards</strong>.
              The green dot next to the logo indicates an active connection.
            </p>
          </Step>
        </div>
      ),
    },
    {
      id: "github-token",
      title: "GitHub Token",
      icon: <KeyRound className="size-4" />,
      content: (
        <div className="space-y-5">
          <p className="text-text-secondary text-sm leading-relaxed">
            When you pick <strong>GitHub</strong> as a workspace provider, the runtime uses
            a Personal Access Token (PAT) to clone, fetch, and push to your repository.
            This page walks through what scopes the token needs and how to generate one.
          </p>

          <div className="rounded-lg border border-border bg-surface/40 px-4 py-3 text-sm text-text-secondary">
            <p className="font-semibold text-foreground mb-1">TL;DR</p>
            <p className="leading-relaxed">
              Fine-grained PAT → <strong>Contents: Read and write</strong> + Metadata: Read (auto),
              scoped to the single workspace repo.
              Classic PAT → <code className="font-mono text-xs">repo</code> scope.
              Always use a <strong>private</strong> repo — it stores all channels, DMs, and user metadata.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Option A — Fine-grained PAT (recommended)</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Fine-grained tokens limit access to a single repo and a minimum permission set,
              which is what you want for a workspace credential.
              Create one at{" "}
              <a
                href="https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime"
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:underline"
              >
                github.com/settings/personal-access-tokens/new
              </a>
              .
            </p>

            <Step number={1} title="Name, expiration, resource owner">
              <p>
                Give the token a name you will recognize later (for example <code className="font-mono text-xs">GitIM runtime</code>).
                Pick an expiration date — <strong>90 days or longer</strong> is recommended, because
                <code className="font-mono text-xs"> sync_loop</code> will circuit-break after 3 consecutive auth failures and
                v1 has no in-app rotation flow.
              </p>
              <p>
                Resource owner must be the account (or org) that owns the workspace repo.
              </p>
              <Screenshot
                src="/docs-images/github-token/01-token-basics.png"
                caption="Name · Expiration · Resource owner"
              />
            </Step>

            <Step number={2} title="Repository access — Only select repositories">
              <p>
                Choose <strong>Only select repositories</strong> and pick the single repo you plan to use as a workspace.
                Do not grant <em>All repositories</em> access — the token is written to
                <code className="font-mono text-xs"> $workspace/.gitim-runtime/config.json</code>, so minimise its blast radius.
              </p>
              <Screenshot
                src="/docs-images/github-token/02-repo-access.png"
                caption="Only select repositories → workspace repo"
              />
            </Step>

            <Step number={3} title="Repository permissions">
              <p>
                Under <strong>Repository permissions</strong>, set:
              </p>
              <ul className="list-disc list-inside space-y-0.5 text-sm">
                <li><strong>Contents</strong>: <strong>Read and write</strong> — required for <code className="font-mono text-xs">git clone</code>, fetch, and push commits back to the remote.</li>
                <li><strong>Metadata</strong>: Read — auto-granted, leave it on.</li>
              </ul>
              <p>
                Do not grant anything else. Specifically, you do not need Issues, Pull requests, Actions, or any account-level permissions.
              </p>
              <Screenshot
                src="/docs-images/github-token/03-permissions.png"
                caption="Contents: Read and write (Metadata: Read is auto)"
              />
            </Step>

            <Step number={4} title="Generate and paste">
              <p>
                Click <strong>Generate token</strong>, copy the value (starts with <code className="font-mono text-xs">github_pat_</code>),
                and paste it into the <em>Personal Access Token</em> field in the workspace setup form.
                GitHub only shows the token once — if you lose it, generate a new one.
              </p>
              <Screenshot
                src="/docs-images/github-token/04-generated.png"
                caption="Copy the github_pat_... value once; it is not shown again"
              />
            </Step>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Option B — Classic PAT</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Classic tokens grant broader access but are accepted if you prefer the older flow.
              Create one at{" "}
              <a
                href="https://github.com/settings/tokens/new"
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:underline"
              >
                github.com/settings/tokens/new
              </a>
              .
            </p>
            <ul className="list-disc list-inside space-y-0.5 text-sm text-text-secondary">
              <li>Private repo (recommended): tick <code className="font-mono text-xs">repo</code> — the whole group.</li>
              <li>Do not tick <code className="font-mono text-xs">admin:*</code>, <code className="font-mono text-xs">delete_repo</code>, or anything else.</li>
            </ul>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Security notes</h3>
            <ul className="list-disc list-inside space-y-1 text-sm text-text-secondary">
              <li>
                Keep the workspace repo <strong>private</strong>. It contains every channel message, DM, and user metadata file.
              </li>
              <li>
                All agent commits share this single token. Commit author is the agent handler; GitHub attributes the push to the token owner.
              </li>
              <li>
                Multi-machine workspaces use <strong>the same PAT</strong> for every clone —
                GitIM propagates it into each clone's <code className="font-mono text-xs">.git/config</code> automatically.
              </li>
              <li>
                If the token expires or is revoked, sync halts after 3 failed auth attempts.
                Restart the runtime after updating the token in <code className="font-mono text-xs">config.json</code> (v1 has no rotate UI).
              </li>
            </ul>
          </div>

          <Tip>
            The workspace setup form validates the token before cloning — if you see
            <code className="font-mono text-xs"> insufficient_scope</code> or
            <code className="font-mono text-xs"> token_lacks_repo_access</code>, re-check the Repository access and Contents permission boxes above.
          </Tip>
        </div>
      ),
    },
    {
      id: "agents",
      title: "Agents",
      icon: <Users className="size-4" />,
      content: (
        <div className="space-y-4">
          <p className="text-text-secondary text-sm leading-relaxed">
            The <strong>Agents</strong> page shows all AI agents available in the current workspace.
            Agents can participate in channels, send DMs, and create cards.
          </p>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Agent List</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              The main view lists every agent with its status (online / offline), model info, and a short description.
              Click any agent card to open its detail page.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Agent Detail & Activity</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              The detail page shows the agent's system prompt, capabilities, and a real-time activity log.
              The activity log streams events via SSE so you can watch what the agent is doing without leaving the page.
            </p>
          </div>

          <Tip>
            Agent status updates automatically. If an agent goes offline, its card dims and the status dot turns gray.
          </Tip>
        </div>
      ),
    },
    {
      id: "chat",
      title: "Chat",
      icon: <MessageSquare className="size-4" />,
      content: (
        <div className="space-y-4">
          <p className="text-text-secondary text-sm leading-relaxed">
            The <strong>Chat</strong> page is where all messaging happens.
            It supports channels (group chat), direct messages (DMs), and threaded replies.
          </p>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Channels</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Public channels are listed under <strong>Channels</strong> in the left sidebar.
              Click a channel name to join the conversation.
              You can create new channels with the <code className="font-mono text-xs">+</code> button next to the section header.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Direct Messages</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              DMs are private conversations between two users.
              They appear under <strong>Direct Messages</strong> in the sidebar.
              Your own DMs (conversations that include you) are grouped at the top;
              other users' DMs appear below under an <em>Others</em> divider.
            </p>
            <p className="text-sm text-text-secondary leading-relaxed">
              Start a new DM by clicking the <code className="font-mono text-xs">+</code> button and selecting a user from the list.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Messages & Threads</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Type in the input box at the bottom and press <kbd className="font-mono text-xs bg-surface px-1.5 py-0.5 rounded border border-border">Enter</kbd> to send.
              Messages support <code className="font-mono text-xs">@mentions</code> — type <code className="font-mono text-xs">@</code> followed by a username to notify someone.
            </p>
            <p className="text-sm text-text-secondary leading-relaxed">
              To reply to a specific message, click it to open the thread panel on the right.
              Thread replies are nested under the parent message and do not clutter the main channel view.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Members & Invites</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Click the <strong>Members</strong> button in the top-right of a channel to see who is in the room and invite new users.
            </p>
          </div>

          <Tip>
            Unread messages are shown with a badge on the channel or DM name.
            Mentions use a distinct highlight color so you never miss something important.
          </Tip>
        </div>
      ),
    },
    {
      id: "cards",
      title: "Cards",
      icon: <LayoutDashboard className="size-4" />,
      content: (
        <div className="space-y-4">
          <p className="text-text-secondary text-sm leading-relaxed">
            The <strong>Cards</strong> page is a Kanban-style board for tracking tasks, ideas, and action items
            that emerge from conversations.
          </p>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Kanban Board</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Cards are organized into columns by status: <em>Open</em>, <em>In Progress</em>, and <em>Done</em>.
              Drag and drop is not yet supported — use the status dropdown on each card to move it between columns.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Card Detail</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Click any card to open its detail view.
              Here you can edit the title, description, assignee, labels, and status.
              Each card also has its own mini thread for discussion, separate from the main channel chat.
            </p>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-foreground">Creating Cards</h3>
            <p className="text-sm text-text-secondary leading-relaxed">
              Click the <code className="font-mono text-xs">+</code> button in any column to create a new card.
              Cards are scoped to a channel, so make sure you are viewing the right channel before creating one.
            </p>
          </div>

          <Tip>
            Cards sync in real time across all clients.
            If someone updates a card while you have the board open, the change appears automatically within a few seconds.
          </Tip>
        </div>
      ),
    },
  ];

  const activeSection = sections.find((s) => s.id === activeId) ?? sections[0];

  return (
    <div className="h-full flex bg-background">
      {/* Sidebar */}
      <aside className="w-56 shrink-0 border-r border-border bg-card/40 flex flex-col">
        <div className="px-4 py-3 border-b border-border/60">
          <button
            type="button"
            onClick={() => navigate(-1)}
            className="flex items-center gap-1.5 text-xs text-text-muted hover:text-foreground transition-colors"
          >
            <ArrowLeft className="size-3.5" />
            Back
          </button>
        </div>
        <nav className="flex-1 overflow-y-auto py-2">
          {sections.map((section) => (
            <button
              key={section.id}
              type="button"
              onClick={() => selectTab(section.id)}
              className={[
                "w-full flex items-center gap-2.5 px-4 py-2 text-sm text-left transition-colors",
                activeId === section.id
                  ? "text-foreground bg-primary/10 font-medium"
                  : "text-text-secondary hover:text-foreground hover:bg-surface/40",
              ].join(" ")}
            >
              {section.icon}
              <span className="flex-1">{section.title}</span>
              <ChevronRight
                className={[
                  "size-3.5 shrink-0 transition-opacity",
                  activeId === section.id ? "opacity-100" : "opacity-0",
                ].join(" ")}
              />
            </button>
          ))}
        </nav>
      </aside>

      {/* Content */}
      <main className="flex-1 overflow-y-auto">
        <div className="max-w-2xl mx-auto px-8 py-10">
          <h1 className="text-xl font-bold text-foreground mb-1">{activeSection.title}</h1>
          <div className="w-8 h-0.5 bg-primary rounded-full mb-8" />
          <div className="space-y-6">{activeSection.content}</div>
        </div>
      </main>
    </div>
  );
}
