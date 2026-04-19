import { useState } from "react";
import { useNavigate } from "react-router";
import {
  ArrowLeft,
  CircleHelp,
  Users,
  MessageSquare,
  LayoutDashboard,
  ChevronRight,
} from "lucide-react";

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

export function DocsPage() {
  const navigate = useNavigate();
  const [activeId, setActiveId] = useState("quickstart");

  const sections: DocSection[] = [
    {
      id: "quickstart",
      title: "Quick Start",
      icon: <CircleHelp className="size-4" />,
      content: (
        <div className="space-y-2">
          <p className="text-text-secondary text-sm leading-relaxed">
            GitIM·Cell is a lightweight chat client that connects to a local GitIM daemon.
            This guide walks you through the essentials to get up and running in minutes.
          </p>

          <Step number={1} title="Connect to the daemon">
            <p>
              On first launch, you will see a setup screen asking for the daemon port.
              The default is <code className="font-mono text-xs bg-surface px-1.5 py-0.5 rounded border border-border">localhost:7374</code>.
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
              onClick={() => setActiveId(section.id)}
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
