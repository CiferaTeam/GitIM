import type { ReactNode } from "react";
import {
  Bullets,
  Callout,
  ChapterLinks,
  Code,
  CodeBlock,
  ConceptGrid,
  FeatureGrid,
  Screenshot,
  Section,
  Steps,
} from "./docs-primitives";
import { ChapterJourney, type DocSectionId } from "./docs-journeys";

export type { DocSectionId } from "./docs-journeys";

interface DocSection {
  id: DocSectionId;
  title: string;
  summary: string;
  content: ReactNode;
}

interface DocGroup {
  id: string;
  title: string;
  sectionIds: DocSectionId[];
}

export const DOC_GROUPS: DocGroup[] = [
  {
    id: "get-started",
    title: "Get started",
    sectionIds: ["quickstart", "workspaces", "github-token"],
  },
  {
    id: "product-guides",
    title: "Product guides",
    sectionIds: [
      "agents",
      "messaging",
      "work-management",
      "automation",
      "quick-sessions",
    ],
  },
  {
    id: "how-it-works",
    title: "How it works",
    sectionIds: ["protocol", "runtime", "distributed"],
  },
  {
    id: "reference",
    title: "Reference",
    sectionIds: ["cli-api", "operations"],
  },
];

export const DOC_SECTIONS: DocSection[] = [
  {
    id: "quickstart",
    title: "Quick Start",
    summary:
      "Create a Git-backed workspace, add an agent, and start working together without deploying a coordination service.",
    content: (
      <>
        <ChapterJourney id="quickstart" />

        <Callout title="Four steps to a working agent team">
          GitIM turns an ordinary Git repository into the shared memory and audit trail for
          people and local coding agents. The web app is the workspace UI; your agents keep
          running in environments you already use.
        </Callout>

        <Section title="Go deeper: the system model">
          <ConceptGrid
            items={[
              {
                name: "Web app",
                role: "The visual workspace for people: onboarding, conversations, cards, agent status, and documentation.",
                detail: "It talks to a local runtime for native agent work, or uses browser mode for repository-backed collaboration.",
              },
              {
                name: "Runtime",
                role: "The local control plane that knows which workspaces and agents exist on this machine.",
                detail: "It starts provider sessions, exposes local APIs, and streams activity back to the web app.",
              },
              {
                name: "Workspace",
                role: "The collaboration boundary shared by a team. One workspace maps to one Git repository.",
                detail: "Its channels, members, cards, boards, flows, and history remain separate from every other workspace.",
              },
              {
                name: "Agent",
                role: "A first-class teammate with a handler, role, provider, model, working clone, and runtime state.",
                detail: "People can mention it, message it, assign cards to it, or invite it into repeatable flows.",
              },
            ]}
          />
        </Section>

        <Section title="Fastest path">
          <p>
            Open{" "}
            <a
              href="https://gitim.io"
              target="_blank"
              rel="noopener noreferrer"
              className="font-medium text-primary hover:underline"
            >
              gitim.io
            </a>
            . Guided onboarding detects your platform, installs the local runtime, and
            walks you through the first workspace. You need Git 2.30+ and, for agent use,
            at least one supported provider CLI installed and signed in.
          </p>
          <p>
            To build the three native binaries yourself, use the repository installer:
          </p>
          <CodeBlock>{`git clone https://github.com/CiferaTeam/GitIM
cd GitIM
./scripts/install-from-source.sh`}</CodeBlock>
          <p>
            Native runtime support targets macOS 12+, recent Linux, and Windows through
            WSL2.
          </p>
        </Section>

        <Steps
          items={[
            {
              title: "Choose how GitIM runs",
              children: (
                <p>
                  Open the hosted web app for a browser-first workspace, or run the GitIM
                  runtime locally for the full agent experience. A local runtime manages
                  provider processes and exposes the workspace to the WebUI.
                </p>
              ),
            },
            {
              title: "Create or open a workspace",
              children: (
                <>
                  <p>
                    For one machine, choose a local folder. For a team or several devices,
                    connect a private GitHub repository. GitIM initializes its folders and
                    records future organizational changes as commits.
                  </p>
                  <p>
                    A Git remote plus local agent environments is enough. There is no GitIM
                    server or database to deploy.
                  </p>
                </>
              ),
            },
            {
              title: "Add your first agent",
              children: (
                <p>
                  Open <strong>Agents</strong>, choose a provider such as Codex, Claude,
                  Gemini, or Kimi, then set a unique handler and role. GitIM verifies that
                  the selected provider and model can answer before it creates durable agent
                  files.
                </p>
              ),
            },
            {
              title: "Start collaborating",
              children: (
                <p>
                  Create a channel, mention the agent with <Code>&lt;@handler&gt;</Code>, or
                  assign it a card. Use Boards for team status, Flows for repeatable
                  processes, and Quick Sessions for a focused one-agent conversation.
                </p>
              ),
            },
          ]}
        />

        <Section title="What to open next">
          <ChapterLinks
            items={[
              {
                id: "agents",
                title: "Agents",
                body: "Add providers, edit roles and model settings, and inspect live activity and usage.",
                to: "/docs?tab=agents",
              },
              {
                id: "messaging",
                title: "Chat",
                body: "Use channels, direct messages, mentions, replies, search, references, and attachments.",
                to: "/docs?tab=messaging",
              },
              {
                id: "work-management",
                title: "Cards & Boards",
                body: "Turn conversations into assigned work and keep status visible across the team.",
                to: "/docs?tab=work-management",
              },
              {
                id: "automation",
                title: "Flows",
                body: "Save a repeatable DAG in Markdown, then track each run and node in Git.",
                to: "/docs?tab=automation",
              },
            ]}
          />
        </Section>
      </>
    ),
  },
  {
    id: "workspaces",
    title: "Workspaces & Setup",
    summary:
      "A workspace is both the collaboration boundary and the Git repository that stores its state.",
    content: (
      <>
        <ChapterJourney id="workspaces" />

        <Callout title="A workspace is a Git repository">
          Channels, direct messages, cards, projects, boards, flow definitions, and user
          metadata live as readable files. Git provides replication, history, attribution,
          and recovery.
        </Callout>

        <Section title="Go deeper: workspace internals">
          <ConceptGrid
            items={[
              {
                name: "Workspace record",
                role: "The local entry shown in the workspace switcher, identified by a stable slug and a display name.",
                detail: "It tells the runtime where the workspace lives and whether it uses a local or hosted Git remote.",
              },
              {
                name: "Repository",
                role: "The durable source of truth for collaboration objects and their history.",
                detail: "A repository is not merely an export: its current files are live state and its commits are the audit trail.",
              },
              {
                name: "Human clone",
                role: "The working copy used for actions performed by the person operating this runtime.",
                detail: "Its local identity determines who authors messages and organizational changes made through the WebUI.",
              },
              {
                name: "Agent clone",
                role: "An isolated working copy owned by one agent handler.",
                detail: "Separate clones keep identities, cursors, provider sessions, and concurrent Git operations from colliding.",
              },
              {
                name: "Member identity",
                role: "A workspace user or agent described by a handler, display name, introduction, and optional labels.",
                detail: "The handler is the stable key used by mentions, assignments, message authorship, and Git commits.",
              },
              {
                name: "Remote",
                role: "The Git endpoint that connects nodes in a distributed workspace.",
                detail: "A local-only workspace can omit it; a multi-machine workspace uses the same private remote as its shared transport.",
              },
            ]}
          />
        </Section>

        <Section title="Choose a workspace mode">
          <FeatureGrid
            items={[
              {
                title: "Local",
                body: (
                  <>
                    Point GitIM at a folder. The runtime creates a local bare repository and
                    a human clone under <Code>.gitim-runtime/human/</Code>. This is the
                    shortest route to a single-machine workspace.
                  </>
                ),
              },
              {
                title: "GitHub",
                body: (
                  <>
                    Connect a private repository with a Personal Access Token. Every node
                    clones the same remote, so people and agents can work from different
                    machines without a central GitIM service.
                  </>
                ),
              },
            ]}
          />
        </Section>

        <Section title="What setup creates">
          <Bullets>
            <li>
              <Code>.gitim-runtime/config.json</Code> stores workspace runtime configuration
              and, in GitHub mode, the source-of-truth token with owner-only file
              permissions.
            </li>
            <li>
              <Code>.gitim-runtime/human/</Code> is the local human clone. Each provisioned
              agent receives its own clone and identity.
            </li>
            <li>
              <Code>users/&lt;handler&gt;.meta.yaml</Code> records participants, while
              <Code>.gitim/me.json</Code> identifies the actor in a clone.
            </li>
            <li>
              Workspace content is isolated. Switching workspaces changes the entire team,
              conversation, and work-management context.
            </li>
          </Bullets>
        </Section>

        <Section title="Setup boundaries">
          <p>
            GitHub mode validates the token and repository before cloning. Cloud-synced
            folders such as iCloud Drive, Dropbox, Google Drive, and OneDrive are rejected
            because two synchronization systems must not rewrite the same Git working tree.
            Native Windows setup is outside the current scope; use WSL2 for the local
            runtime.
          </p>
          <Callout title="Changing the remote" tone="neutral">
            Local-to-GitHub migration and changing an existing remote URL are rebuild
            operations in the current version. Create a new workspace instead of editing a
            live clone in place.
          </Callout>
        </Section>
      </>
    ),
  },
  {
    id: "github-token",
    title: "GitHub Token",
    summary:
      "Give GitIM the narrowest repository permission it needs to clone, fetch, commit, and push workspace state.",
    content: (
      <>
        <ChapterJourney id="github-token" />

        <Callout title="Recommended permission">
          Create a fine-grained Personal Access Token for one private workspace repository.
          Set <strong>Contents: Read and write</strong>; GitHub grants Metadata: Read
          automatically. No Issues, Pull requests, Actions, or account permissions are
          required.
        </Callout>

        <Section title="Go deeper: credential boundaries">
          <ConceptGrid
            items={[
              {
                name: "Personal Access Token",
                role: "The credential GitIM uses for repository checks, clone, fetch, and push.",
                detail: "It represents the GitHub account operating the runtime; it is not an individual agent password.",
              },
              {
                name: "Resource owner",
                role: "The user or organization that owns the repository selected for the workspace.",
                detail: "The token owner must be allowed to grant access to that repository, including organization approval when required.",
              },
              {
                name: "Repository access",
                role: "The list of repositories the token may reach.",
                detail: "Choose one dedicated private workspace repository instead of granting access to every repository the account can see.",
              },
              {
                name: "Permission set",
                role: "The operations the token may perform inside the selected repository.",
                detail: "Contents read/write is sufficient for GitIM; metadata read is supplied automatically by GitHub.",
              },
              {
                name: "Runtime credential store",
                role: "The owner-readable local workspace configuration that holds the source token.",
                detail: "This file stays outside Git so collaboration history never contains the reusable credential.",
              },
              {
                name: "Clone credentials",
                role: "The derived authentication attached to each local clone.",
                detail: "The runtime refreshes these from the workspace token when it starts and when it provisions another agent.",
              },
            ]}
          />
        </Section>

        <Section title="Fine-grained token setup" eyebrow="Recommended">
          <p>
            Start at{" "}
            <a
              href="https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime"
              target="_blank"
              rel="noopener noreferrer"
              className="font-medium text-primary hover:underline"
            >
              GitHub fine-grained token settings
            </a>
            .
          </p>
          <Steps
            items={[
              {
                title: "Set the owner and expiration",
                children: (
                  <>
                    <p>
                      Create the token from GitHub settings. Choose the account or
                      organization that owns the repository, give the token a recognizable
                      name, and set an expiration you can operate.
                    </p>
                    <Screenshot
                      src="/docs-images/github-token/01-token-basics.png"
                      caption="Name, expiration, and resource owner"
                    />
                  </>
                ),
              },
              {
                title: "Select one repository",
                children: (
                  <>
                    <p>
                      Choose <strong>Only select repositories</strong> and select the private
                      repository that will hold the workspace.
                    </p>
                    <Screenshot
                      src="/docs-images/github-token/02-repo-access.png"
                      caption="Limit access to the workspace repository"
                    />
                  </>
                ),
              },
              {
                title: "Grant repository contents access",
                children: (
                  <>
                    <p>
                      Under Repository permissions, set <strong>Contents</strong> to
                      <strong> Read and write</strong>. Leave every unrelated permission
                      disabled.
                    </p>
                    <Screenshot
                      src="/docs-images/github-token/03-permissions.png"
                      caption="Contents: Read and write; Metadata: Read"
                    />
                  </>
                ),
              },
              {
                title: "Generate, copy, and connect",
                children: (
                  <>
                    <p>
                      Copy the <Code>github_pat_…</Code> value immediately and paste it into
                      workspace setup. GitHub shows it only once.
                    </p>
                    <Screenshot
                      src="/docs-images/github-token/04-generated.png"
                      caption="Copy the generated token before leaving the page"
                    />
                  </>
                ),
              },
            ]}
          />
        </Section>

        <Section title="Classic tokens">
          <p>
            A classic PAT also works. Grant the <Code>repo</Code> scope for a private
            workspace, but understand that it covers more repositories than a fine-grained
            token. Do not add admin or repository deletion scopes. Create one from{" "}
            <a
              href="https://github.com/settings/tokens/new"
              target="_blank"
              rel="noopener noreferrer"
              className="font-medium text-primary hover:underline"
            >
              GitHub classic token settings
            </a>
            .
          </p>
        </Section>

        <Section title="Storage and recovery">
          <Bullets>
            <li>
              The token source of truth is <Code>.gitim-runtime/config.json</Code>; derived
              clone URLs are refreshed from that value when the runtime starts and when an
              agent is added.
            </li>
            <li>
              After three consecutive authentication failures, Git sync pauses remote
              operations to protect the GitHub rate limit.
            </li>
            <li>
              To rotate a token today, update the workspace config and restart the runtime
              so every clone receives the new credential.
            </li>
          </Bullets>
        </Section>
      </>
    ),
  },
  {
    id: "agents",
    title: "Agents & Providers",
    summary:
      "Each agent has a durable Git identity, an isolated working copy, provider configuration, and observable runtime state.",
    content: (
      <>
        <ChapterJourney id="agents" />

        <Callout title="Provisioning is transactional">
          GitIM runs a provider-specific preflight before creating a clone, metadata, or
          runtime state. If the exact provider and model cannot answer, setup stops without
          leaving a partial agent behind.
        </Callout>

        <Section title="Go deeper: agent internals">
          <ConceptGrid
            items={[
              {
                name: "Handler",
                role: "The stable lowercase identity used in mentions, assignments, messages, and Git authorship.",
                detail: "Display names can change; the handler remains the durable address other workspace objects point to.",
              },
              {
                name: "Role profile",
                role: "The human-readable name, introduction, system prompt, and capability labels that describe the agent.",
                detail: "This is the team-facing definition of what the agent is for, separate from the model that happens to power it.",
              },
              {
                name: "Provider adapter",
                role: "The bridge between GitIM and a local agent harness such as Codex, Claude, Kimi, or Hermes.",
                detail: "It translates one owned turn into the provider’s CLI or profile conventions and normalizes activity and results.",
              },
              {
                name: "Model settings",
                role: "The provider-specific model, reasoning effort, LLM provider, and environment used by this agent.",
                detail: "Some settings can change only while the agent is stopped because an active session is bound to the old model configuration.",
              },
              {
                name: "Working clone",
                role: "The agent’s isolated view of the workspace repository.",
                detail: "It gives the agent its own Git identity, local files, cursor, and safe place to synchronize without sharing a working tree.",
              },
              {
                name: "Provider session",
                role: "The resumable conversation state maintained between successive owned turns.",
                detail: "A reset clears model-bound context without removing the agent’s Git identity, configuration, or historical usage.",
              },
              {
                name: "Runtime state",
                role: "The current operational status: starting, running, stopped, processing, retrying, or failed.",
                detail: "The Agents page combines this state with recent activity so operators can distinguish an idle agent from a broken one.",
              },
              {
                name: "Usage record",
                role: "Daily and lifetime token totals accumulated for the agent.",
                detail: "Usage belongs to the agent rather than one provider session, so resetting a session does not erase operational history.",
              },
            ]}
          />
        </Section>

        <Section title="Supported providers">
          <FeatureGrid
            items={[
              { title: "Native CLI", body: "Claude, Codex, Gemini, Cursor, OpenCode, Pi, and OpenClaw." },
              { title: "ACP & profiles", body: "Kimi and Hermes, including an isolated Hermes profile per GitIM agent." },
              { title: "Per-agent model", body: "Choose a model where the provider supports per-invocation or profile configuration." },
              { title: "Stable identity", body: "A lowercase handler names the user record, Git author, mentions, assignments, and routing target." },
            ]}
          />
        </Section>

        <Section title="Agent lifecycle">
          <Steps
            items={[
              {
                title: "Preflight",
                children: "The runtime checks the local CLI, authentication, provider connectivity, and requested model.",
              },
              {
                title: "Provision",
                children: "GitIM creates an agent clone, registers its user metadata, and writes the provider profile and local configuration.",
              },
              {
                title: "Run",
                children: "A polling loop watches routed work, invokes the provider, commits replies, and syncs them through Git.",
              },
              {
                title: "Stop, edit, or burn",
                children: "Stop an agent before changing model-bound settings. Burning removes its local runtime resources; shared Git history remains auditable.",
              },
            ]}
          />
        </Section>

        <Section title="Configuration and visibility">
          <Bullets>
            <li>
              Edit the display name, model, reasoning effort where supported, system prompt,
              environment variables, and a local <Code>.env</Code> file.
            </li>
            <li>
              The agent detail page streams activity and exposes session status, recent
              events, and token usage without opening its terminal.
            </li>
            <li>
              Usage is accumulated per agent and by day, independent of provider-session
              resets.
            </li>
            <li>
              Secrets stay in runtime configuration or the agent-local environment; they
              are not committed with workspace content.
            </li>
          </Bullets>
        </Section>
      </>
    ),
  },
  {
    id: "messaging",
    title: "Messaging",
    summary:
      "Conversation feels like an IM, while every message remains readable, referenceable, and attributable in Git.",
    content: (
      <>
        <ChapterJourney id="messaging" />

        <Section title="Go deeper: message anatomy">
          <ConceptGrid
            items={[
              {
                name: "Channel",
                role: "A named group conversation with an introduction, creator, member list, and archive state.",
                detail: "Channels are the default place for durable team discussion and can be grouped under an optional project.",
              },
              {
                name: "Direct message",
                role: "A private two-member conversation addressed by the participants’ handlers.",
                detail: "It uses the same message and reply model as a channel but limits visibility to the two participants.",
              },
              {
                name: "Message",
                role: "An authored body with a stable line number and timestamp.",
                detail: "The line number is the address used by replies, references, incremental reads, and unread tracking.",
              },
              {
                name: "Reply chain",
                role: "A thread formed when a message points to the line number of its parent.",
                detail: "Replies preserve conversational context without introducing a separate hidden thread database.",
              },
              {
                name: "Mention",
                role: "An explicit <@handler> address embedded in the message body.",
                detail: "Mentions help route work to the intended agent and provide a visible, portable record of who was called.",
              },
              {
                name: "Recipient set",
                role: "The agents eligible to receive a channel or DM message as new work.",
                detail: "It combines conversation ownership, reply participation, and mentions to avoid every agent responding to every line.",
              },
              {
                name: "Reference",
                role: "A canonical link to another message, card, channel, or Quick Session.",
                detail: "References render previews and let a reader move between conversation, work, and focused-session context.",
              },
              {
                name: "Attachment",
                role: "A workspace-scoped asset referenced from message text.",
                detail: "The reference travels with the conversation while the runtime resolves and verifies the underlying file for the current node.",
              },
            ]}
          />
        </Section>

        <Section title="Conversation surfaces">
          <FeatureGrid
            items={[
              { title: "Channels", body: "Group conversations with members, threaded replies, archive state, and optional project grouping." },
              { title: "Direct messages", body: "A canonical two-person thread whose filename is derived from both handlers." },
              { title: "Card discussions", body: "A dedicated thread beside each card, connected to the work item without cluttering the channel." },
              { title: "Search & references", body: "Search visible message bodies and link directly to messages, cards, and Quick Sessions." },
            ]}
          />
        </Section>

        <Section title="Mentions and replies">
          <p>
            Use <Code>&lt;@handler&gt;</Code> to mention a person or agent. A reply records
            the parent line, so the thread structure survives every UI and Git client.
            Attachments are stored as workspace assets and referenced from messages.
          </p>
        </Section>

        <Section title="Recipient routing">
          <p>
            Agents do not all answer every new line. For a channel message, GitIM combines
            three recipient signals: the channel creator, authors already present in the
            parent chain, and explicit mentions. A direct message routes to its two
            participants. Older records without recipient metadata fall back safely to
            broadcast behavior.
          </p>
          <CodeBlock>{`Human message
  ├─ channel owner
  ├─ authors in the reply chain
  └─ explicit <@mentions>
          ↓
Only matching agent loops receive the turn`}</CodeBlock>
        </Section>

        <Section title="Archive and visibility">
          <p>
            Archiving moves a conversation into the repository archive rather than erasing
            it. Channel archive also carries channel-owned cards with it, while manually
            archived cards remain independently archived. Departed users and membership
            checks continue to constrain reads and writes.
          </p>
        </Section>
      </>
    ),
  },
  {
    id: "work-management",
    title: "Work Management",
    summary:
      "Turn conversation into structured work using cards, projects, boards, and labels—without leaving the repository.",
    content: (
      <>
        <ChapterJourney id="work-management" />

        <Callout title="Cards, projects, boards, and labels">
          These are separate views over one Git-backed organization. Cards capture work;
          projects organize channels; boards summarize a person or agent; labels connect
          capabilities and work across objects.
        </Callout>

        <Section title="Go deeper: work objects">
          <ConceptGrid
            items={[
              {
                name: "Project",
                role: "A lightweight grouping for related channels.",
                detail: "It organizes navigation and filtering without changing who may read a channel or which agents receive its messages.",
              },
              {
                name: "Card",
                role: "A channel-owned work item with a title, description, status, labels, and optional assignee.",
                detail: "Cards turn an outcome from conversation into a durable object that can be filtered and moved through a Kanban.",
              },
              {
                name: "Status",
                role: "The card’s workflow position: todo, doing, or done.",
                detail: "Changing status updates the shared card rather than creating a separate board-only representation.",
              },
              {
                name: "Assignee",
                role: "The member currently responsible for moving a card forward.",
                detail: "The assignee can be a person or agent handler; assignment is independent from who created or discussed the card.",
              },
              {
                name: "Card discussion",
                role: "A message thread attached directly to one card.",
                detail: "It keeps investigation and decisions beside the task while canonical references connect it back to channel messages.",
              },
              {
                name: "Board",
                role: "A personal or agent-authored Markdown status surface.",
                detail: "Boards communicate priorities and handoff context; they complement the shared card Kanban rather than duplicating it.",
              },
              {
                name: "Label",
                role: "A validated capability or classification shared across cards, boards, members, and flow requirements.",
                detail: "Labels support filtering and all-of agent matching, so work can be connected to members with the right declared capabilities.",
              },
              {
                name: "Archive state",
                role: "A reversible removal from the active work surface.",
                detail: "Cards can be archived directly or follow their channel into archive while remaining inspectable in repository history.",
              },
            ]}
          />
        </Section>

        <Section title="Cards">
          <Bullets>
            <li>
              Every card belongs to a channel and moves through <Code>todo</Code>,
              <Code>doing</Code>, and <Code>done</Code>.
            </li>
            <li>
              Set title, description, assignee, labels, status, and discuss the work in its
              attached thread.
            </li>
            <li>
              The Kanban supports drag-and-drop and filters for channel, project, label,
              assignee, and work assigned to you.
            </li>
            <li>
              When labels describe the required work, GitIM can suggest agents whose own
              labels match.
            </li>
          </Bullets>
        </Section>

        <Section title="Projects">
          <p>
            A project groups related channels in the Chat sidebar and provides a matching
            Cards filter. It is intentionally lightweight: assigning a channel to a project
            does not alter routing, permissions, archive rules, flows, or card storage.
          </p>
        </Section>

        <Section title="Boards and labels">
          <FeatureGrid
            items={[
              {
                title: "Boards",
                body: "A Git-backed personal or agent status page for priorities, progress, and handoff context.",
              },
              {
                title: "Unified labels",
                body: "Cards, boards, users, and flow-node requirements share one validated label vocabulary.",
              },
            ]}
          />
        </Section>
      </>
    ),
  },
  {
    id: "automation",
    title: "Flows & Automation",
    summary:
      "Encode repeatable coordination as repository files, then observe each run and reminder as it advances.",
    content: (
      <>
        <ChapterJourney id="automation" />

        <Callout title="Templates, runs, schedules, and timers">
          Flows describe multi-agent work. Runs record the state of one execution. Cron
          schedules repeat work, while timers let an agent resume a specific task later.
        </Callout>

        <Section title="Go deeper: automation objects">
          <ConceptGrid
            items={[
              {
                name: "Flow template",
                role: "A reusable team process that names a set of nodes and the dependencies between them.",
                detail: "The template describes how work should be coordinated; it does not represent one specific execution.",
              },
              {
                name: "Node",
                role: "One responsibility inside a flow, such as calling an agent, opening a channel discussion, waiting for review, or waiting for a signal.",
                detail: "A node has a stable id, type, owner or participants, upstream needs, and optional capability labels.",
              },
              {
                name: "Dependency",
                role: "A directed relationship declaring which nodes must finish before another node is ready.",
                detail: "Together these relationships form the DAG shown in the flow editor and run detail page.",
              },
              {
                name: "Node prompt",
                role: "The Markdown instructions associated with one node id.",
                detail: "Keeping prompts beside the flow definition makes the process readable and reviewable as a team artifact.",
              },
              {
                name: "Flow run",
                role: "One execution of a template, bound to the channel where its coordination happens.",
                detail: "A template may have many runs over time; each run keeps its own overall status and timestamps.",
              },
              {
                name: "Run-node state",
                role: "The progress record for one node inside one run.",
                detail: "It advances from pending to in progress and then to done, failed, or skipped, with an optional actor and result reference.",
              },
              {
                name: "Cron schedule",
                role: "A recurring trigger that asks an agent to perform a prompt on a schedule and timezone.",
                detail: "Enabled state and execution history make recurring work observable rather than an invisible operating-system task.",
              },
              {
                name: "One-shot timer",
                role: "A short-lived reminder owned by one agent and anchored to the work it should revisit.",
                detail: "Timers are local runtime state for deferred attention, while flow and cron definitions are durable team objects.",
              },
            ]}
          />
        </Section>

        <Section title="Flow templates">
          <p>
            A flow lives at <Code>flows/&lt;slug&gt;/index.md</Code>. YAML frontmatter
            defines a directed acyclic graph and the Markdown body gives each node its
            working prompt. The UI renders the graph and exposes the template to agents
            through the runtime API.
          </p>
          <CodeBlock>{`---
schema_version: 1
slug: release-review
name: Release review
description: Inspect a release before publication
created_by: coordinator
created_at: 2026-07-30T09:00:00Z
nodes:
  - id: inspect
    type: agent_mention
    owner: reviewer
    needs: []
  - id: publish
    type: agent_mention
    owner: coordinator
    needs: [inspect]
---

## inspect
Review the release candidate and record blockers.`}</CodeBlock>
        </Section>

        <Section title="Flow runs">
          <p>
            Starting a flow creates a run bound to a channel. Nodes move forward through
            <Code>pending</Code>, <Code>in_progress</Code>, and a terminal state of
            <Code>done</Code>, <Code>failed</Code>, or <Code>skipped</Code>. The run closes
            as done, failed, or cancelled. Its state is stored at
            <Code>flows/&lt;slug&gt;/runs/&lt;run_id&gt;/state.yaml</Code>.
          </p>
        </Section>

        <Section title="Cron and one-shot timers">
          <FeatureGrid
            items={[
              {
                title: "Cron",
                body: "Define recurring agent work with a schedule, timezone, prompt, enabled state, and execution history.",
              },
              {
                title: "Timer",
                body: "An agent can register a one-shot reminder from 10 seconds to 24 hours. Each agent may keep up to three pending timers.",
              },
            ]}
          />
          <CodeBlock>{`gitim timer set 20m "card:42" --note "Check the CI result"
gitim flow start release-review --channel engineering`}</CodeBlock>
        </Section>
      </>
    ),
  },
  {
    id: "quick-sessions",
    title: "Quick Sessions",
    summary:
      "Open a focused, Git-synced conversation with one assigned agent and carry the result back into the wider workspace.",
    content: (
      <>
        <ChapterJourney id="quick-sessions" />

        <Callout title="Focused agent conversations">
          A Quick Session is narrower than a channel and more durable than a disposable
          prompt. It has an assigned agent, explicit turns, retry ownership, a title,
          summary, and an archive lifecycle.
        </Callout>

        <Section title="Go deeper: session internals">
          <ConceptGrid
            items={[
              {
                name: "Quick Session",
                role: "The durable container for one focused human-to-agent conversation.",
                detail: "It owns the assigned agent, lifecycle status, transcript, title, summary, and active-turn metadata.",
              },
              {
                name: "Assigned agent",
                role: "The single agent authorized to claim and answer actionable turns in the session.",
                detail: "Keeping one owner makes the interaction predictable and avoids the group-response behavior of an open channel.",
              },
              {
                name: "Transcript",
                role: "The ordered human and agent messages exchanged inside the session.",
                detail: "It uses the same line-addressable thread format as other conversations, so individual lines can be referenced elsewhere.",
              },
              {
                name: "Turn",
                role: "One human input and the agent work expected in response.",
                detail: "Additional human input can queue behind an active turn instead of overwriting the message the agent is processing.",
              },
              {
                name: "Attempt claim",
                role: "The temporary ownership token for an agent attempt on a particular input line.",
                detail: "It prevents a delayed or retried process from publishing a stale answer after newer work has taken ownership.",
              },
              {
                name: "Title",
                role: "A concise description generated or set for the session.",
                detail: "The title turns a raw request into a recognizable item in the Quick Sessions hub and later references.",
              },
              {
                name: "Handoff summary",
                role: "Durable compressed context written after meaningful progress.",
                detail: "It allows the session to resume after provider context is reset without replaying an unbounded transcript.",
              },
              {
                name: "Session reference",
                role: "A canonical session or session-line address that can appear in messages and cards.",
                detail: "It carries focused work back into the shared team context without copying the entire exchange.",
              },
            ]}
          />
        </Section>

        <Section title="When to use one">
          <FeatureGrid
            items={[
              { title: "Investigate", body: "Ask one agent to inspect a problem without starting a team-wide channel thread." },
              { title: "Draft", body: "Develop a proposal or artifact through several focused turns." },
              { title: "Handoff", body: "Summarize the outcome, then reference the session from a channel, card, or future prompt." },
              { title: "Recover", body: "Retry an interrupted turn without letting stale attempts overwrite newer work." },
            ]}
          />
        </Section>

        <Section title="Lifecycle and storage">
          <p>
            Session metadata lives in
            <Code>quick-sessions/&lt;session_id&gt;/session.meta.yaml</Code>; the exchange
            lives beside it in <Code>discussion.thread</Code>. Runtime turn claims prevent
            two attempts from owning the same input. Archiving moves both files under
            <Code>archive/quick-sessions/</Code>, and unarchive restores the session.
          </p>
          <p>
            Use references such as <Code>session:&lt;id&gt;</Code> or
            <Code>session:&lt;id&gt;:L000007</Code> to bring the whole session or one line
            into another conversation.
          </p>
        </Section>
      </>
    ),
  },
  {
    id: "protocol",
    title: "Protocol & Storage",
    summary:
      "The product is built on small, mergeable text files whose structure is usable without the WebUI.",
    content: (
      <>
        <ChapterJourney id="protocol" />

        <Section title="Go deeper: protocol objects">
          <ConceptGrid
            items={[
              {
                name: "Handler",
                role: "The protocol-level identity of a person or agent.",
                detail: "It appears in user metadata, message authorship, mentions, assignments, and clone identity.",
              },
              {
                name: "Thread file",
                role: "An append-oriented text document containing one conversation.",
                detail: "Channels, direct messages, card discussions, and Quick Sessions use the same core message grammar in different locations.",
              },
              {
                name: "Message entry",
                role: "A stable line id, parent id, author, UTC timestamp, and body.",
                detail: "The header fields provide the minimum structure needed for replies, cursors, attribution, and deterministic merging.",
              },
              {
                name: "Parent link",
                role: "The line-number pointer that connects a reply to its parent.",
                detail: "A zero parent means a root message; following parent links reconstructs a reply chain without a separate thread id.",
              },
              {
                name: "Metadata document",
                role: "A small YAML or Markdown-frontmatter file describing an object around the thread.",
                detail: "Channel membership, card state, project identity, flow structure, and session lifecycle stay readable beside their content.",
              },
              {
                name: "Canonical reference",
                role: "A portable text address for another protocol object or line.",
                detail: "References remain meaningful in the WebUI, CLI, provider prompts, Git diffs, and plain-text review.",
              },
              {
                name: "Commit",
                role: "The atomic audit unit for a validated organizational change.",
                detail: "A commit records who changed which protocol files and allows ordinary Git tools to inspect or recover that state.",
              },
              {
                name: "Archive tree",
                role: "The repository area for inactive objects that should no longer appear in primary views.",
                detail: "Moving rather than deleting preserves inspectability and supports object-specific unarchive operations.",
              },
            ]}
          />
        </Section>

        <Section title="One line is one message">
          <CodeBlock>{`[L000042][P000000][@alice][20260730T093015Z] Ship the first pass.
[L000043][P000042][@reviewer][20260730T093201Z] Reviewing now.`}</CodeBlock>
          <p>
            <Code>L</Code> is the stable line number, <Code>P</Code> points to a parent
            message or zero, <Code>@</Code> names the author, and the final field is a UTC
            timestamp. A following physical line without an <Code>[L…]</Code> prefix
            continues the previous message.
          </p>
        </Section>

        <Section title="Repository map">
          <CodeBlock>{`users/<handler>.meta.yaml
channels/<channel>.meta.yaml
channels/<channel>.thread
channels/<channel>/cards/<card-id>/
projects/<slug>.meta.yaml
showboards/<handler>/board.md
flows/<slug>/index.md
flows/<slug>/runs/<run-id>/state.yaml
crons/<name>/spec.yaml
quick-sessions/<session-id>/
archive/...`}</CodeBlock>
        </Section>

        <Section title="Why text plus Git">
          <FeatureGrid
            items={[
              { title: "Inspectable", body: "Open the repository with ordinary editors and Git tools; the source of truth is not hidden in a database." },
              { title: "Auditable", body: "Every write has an author, timestamp, diff, and commit history." },
              { title: "Replicable", body: "Git fetch and push synchronize nodes without a proprietary workspace backend." },
              { title: "Recoverable", body: "Archive, history, branches, bundles, and conflict resolution keep organizational state durable." },
            ]}
          />
        </Section>
      </>
    ),
  },
  {
    id: "runtime",
    title: "Runtime & Sync",
    summary:
      "Understand the process boundary between the WebUI, local agent execution, the protocol daemon, and Git.",
    content: (
      <>
        <ChapterJourney id="runtime" />

        <Callout title="Runtime → daemon → Git">
          The runtime orchestrates agents and exposes HTTP to the WebUI. The daemon validates
          and writes protocol operations. Git stores and synchronizes the resulting files.
        </Callout>

        <Section title="Go deeper: runtime internals">
          <ConceptGrid
            items={[
              {
                name: "WebUI client",
                role: "The interactive surface that turns workspace objects and runtime state into pages, editors, and diagnostics.",
                detail: "It requests operations and renders events; it does not become the durable source of truth for collaboration.",
              },
              {
                name: "Runtime service",
                role: "The local coordinator for workspaces, agents, provider readiness, usage, assets, and remote nodes.",
                detail: "It is the HTTP boundary used by the WebUI and runtime CLI, and the lifecycle owner of agent processes.",
              },
              {
                name: "Daemon",
                role: "The workspace protocol service that applies identity, permission, validation, and object rules.",
                detail: "All native collaboration writes pass through it before files are committed.",
              },
              {
                name: "Agent loop",
                role: "The long-lived worker that detects work routed to one agent.",
                detail: "It owns the agent cursor, prepares bounded context, invokes a provider session, and returns results through protocol operations.",
              },
              {
                name: "Provider session",
                role: "The execution boundary for one local agent harness.",
                detail: "It reports activity and a result, supports cancellation, and may preserve provider-specific conversation state across turns.",
              },
              {
                name: "Git synchronizer",
                role: "The loop that commits local protocol changes and reconciles them with the active remote branch.",
                detail: "It handles supported conflicts, authentication failure cadence, and branch transitions without moving business rules into Git hooks.",
              },
              {
                name: "Event stream",
                role: "The live feed of workspace mutations, agent activity, connection state, and usage updates.",
                detail: "It makes the UI responsive while normal reads remain available for recovery after a disconnect.",
              },
              {
                name: "Workspace configuration",
                role: "The local runtime record containing paths, provider mode, remote information, and protected credentials.",
                detail: "It lets the runtime reconstruct managed workspaces after restart without committing machine-specific configuration.",
              },
            ]}
          />
        </Section>

        <Section title="Request path">
          <CodeBlock>{`WebUI / runtime CLI
        ↓ HTTP + SSE
gitim-runtime ── provider sessions
        ↓ local client
gitim-daemon ── validation, reads, writes
        ↓
working tree ── commit ── fetch / merge / push`}</CodeBlock>
        </Section>

        <Section title="What each layer owns">
          <FeatureGrid
            items={[
              { title: "WebUI", body: "Navigation, editing, local browser mode, activity streams, and visual work surfaces." },
              { title: "Runtime", body: "Workspaces, provider preflight, agent processes, polling, usage, Fleet, assets, and HTTP APIs." },
              { title: "Daemon", body: "Identity, permissions, protocol validation, message and work handlers, reconciliation, and events." },
              { title: "Git sync", body: "Commits, remote synchronization, conflict repair, line renumbering, and long-history epoch rotation." },
            ]}
          />
        </Section>

        <Section title="Agent execution">
          <p>
            Each provider exposes a session abstraction with events, result, cancellation,
            and abort. The agent loop polls for routed changes, formats the owned work as
            context, invokes the provider, and returns its output through normal GitIM
            writes. Server-sent events update the UI without making the UI the source of
            truth.
          </p>
        </Section>

        <Section title="Sync safety">
          <Bullets>
            <li>Writes are validated before commit, then checked again when read.</li>
            <li>Conflicting message lines can be renumbered and replayed during merge.</li>
            <li>Authentication failures trip a sync circuit breaker instead of retrying indefinitely.</li>
            <li>Very long histories can rotate to a new epoch while sealed branches remain immutable and recoverable.</li>
          </Bullets>
        </Section>
      </>
    ),
  },
  {
    id: "distributed",
    title: "Distributed, Browser & Mobile",
    summary:
      "Run the same workspace from local desktops, remote agent machines, browsers, and small WASM-capable devices.",
    content: (
      <>
        <ChapterJourney id="distributed" />

        <Callout title="No coordination service to deploy">
          On one machine, specify a folder. For distributed work, create a GitIM repository
          and run agents in local environments such as Codex on whichever nodes you choose.
          Git is the shared transport and record—there is no additional GitIM server,
          database, queue, or cluster to operate.
        </Callout>

        <Section title="Go deeper: node roles">
          <ConceptGrid
            items={[
              {
                name: "Shared Git remote",
                role: "The common repository endpoint used by every trusted participant node.",
                detail: "It transports collaboration state and preserves history without becoming a GitIM-specific application server.",
              },
              {
                name: "Node",
                role: "A workstation, server, or mobile/browser environment participating in the workspace.",
                detail: "Each node can have a different role: human UI, agent execution, observation, or lightweight repository access.",
              },
              {
                name: "Local clone",
                role: "The node’s independent copy of workspace state.",
                detail: "A clone enables offline reads and local commits; Git synchronization reconciles it with changes from other nodes.",
              },
              {
                name: "Runtime node",
                role: "A node capable of running native provider CLIs and persistent agent loops.",
                detail: "Place it near the code, credentials, tools, and compute the agent needs rather than centralizing every agent on one host.",
              },
              {
                name: "WASM client",
                role: "The protocol and Git-safe logic compiled for a browser or mobile-capable environment.",
                detail: "It supports readable, validated workspace interaction without giving the browser native process execution.",
              },
              {
                name: "Fleet entry",
                role: "A configured view of another runtime that can be observed or targeted from the current one.",
                detail: "Fleet makes remote agents visible as managed nodes while each runtime continues to own its local processes.",
              },
              {
                name: "Tunnel",
                role: "A temporary path to a remote runtime’s local HTTP surface.",
                detail: "It is useful when a runtime should remain locally bound and reachable only through an authenticated transport such as SSH.",
              },
              {
                name: "Workspace asset",
                role: "An attachment published on one node and addressed by a canonical reference.",
                detail: "Other nodes resolve and verify that reference instead of assuming the same absolute filesystem path exists everywhere.",
              },
            ]}
          />
        </Section>

        <Section title="Node shapes">
          <FeatureGrid
            items={[
              { title: "Local workstation", body: "WebUI, runtime, daemon, Git clone, and provider CLIs can all run together." },
              { title: "Remote agent node", body: "A machine runs its own clone and agent environment, then synchronizes with the same remote repository." },
              { title: "Browser and mobile", body: "WASM handles protocol parsing, validation, and Git-safe logic so lightweight clients can participate without a native daemon." },
              { title: "Fleet", body: "The runtime can observe and control connected remote runtimes, including resource resolution and tunneled access." },
            ]}
          />
        </Section>

        <Section title="Browser and mobile capability">
          <p>
            The browser implementation reuses <Code>gitim-core</Code> and sync logic through
            <Code>gitim-wasm</Code>, keeping protocol behavior aligned with the native
            daemon. It can read and write repository-backed collaboration state. Native
            provider processes, local shell access, long-running agent loops, and operating
            system credentials still require a runtime node.
          </p>
        </Section>

        <Section title="A practical distributed setup">
          <Steps
            items={[
              { title: "Create one private remote", children: "Use a dedicated GitHub repository as the workspace transport and audit log." },
              { title: "Connect each node", children: "Give every trusted machine its own runtime or browser workspace and its own local clone." },
              { title: "Place agents near their tools", children: "Run Codex, Claude, Kimi, or other providers on the machines that already have the required code, credentials, and compute." },
              { title: "Let Git synchronize", children: "GitIM commits protocol changes, resolves supported conflicts, and streams the resulting state to each UI." },
            ]}
          />
        </Section>
      </>
    ),
  },
  {
    id: "cli-api",
    title: "CLI & API",
    summary:
      "Use the protocol CLI for collaboration and the runtime CLI or HTTP API for local orchestration.",
    content: (
      <>
        <ChapterJourney id="cli-api" />

        <Callout title="Two command surfaces">
          <Code>gitim</Code> reads and writes collaboration state through the daemon.
          <Code>gitim-runtime</Code> manages the runtime, workspaces, agents, Fleet, and
          provider readiness through local HTTP.
        </Callout>

        <Section title="Go deeper: interface boundaries">
          <ConceptGrid
            items={[
              {
                name: "Protocol CLI",
                role: "The gitim command surface for messages, members, work objects, flows, schedules, and sessions.",
                detail: "It speaks in workspace concepts and is the primary shell interface exposed to agents in their working clones.",
              },
              {
                name: "Runtime CLI",
                role: "The gitim-runtime command surface for local orchestration and remote Fleet targets.",
                detail: "It manages processes and machine-owned configuration rather than editing conversation files directly.",
              },
              {
                name: "Runtime HTTP API",
                role: "The local request boundary used by the WebUI and runtime CLI.",
                detail: "Routes are grouped around runtime health, workspaces, agents, the IM gateway, Fleet, and assets.",
              },
              {
                name: "Daemon request API",
                role: "The typed operation boundary for protocol reads and mutations.",
                detail: "The runtime forwards collaboration actions here so WebUI, CLI, and agent callers share the same validation behavior.",
              },
              {
                name: "JSON output",
                role: "A machine-readable representation of CLI responses and errors.",
                detail: "Agents and scripts should prefer structured output when they need ids, cursors, or stable branching on error classes.",
              },
              {
                name: "Exit class",
                role: "The runtime CLI distinction between success, client/network failure, permanent server rejection, and transient server failure.",
                detail: "The class lets automation decide whether to fix input, retry later, or stop without parsing prose.",
              },
              {
                name: "SSE stream",
                role: "The event channel for activity and state changes that should appear without polling every resource.",
                detail: "Clients still recover from a missed stream by reading current state through normal HTTP endpoints.",
              },
              {
                name: "Canonical reference",
                role: "The text-level contract that connects command output to later messages and operations.",
                detail: "A CLI-created card, message, asset, or session can return a reference that another tool can pass through unchanged.",
              },
            ]}
          />
        </Section>

        <Section title="Protocol CLI">
          <CodeBlock>{`gitim channels
gitim read engineering
gitim send engineering "<@reviewer> inspect card:42"
gitim search "release blocker"
gitim card ls --channel engineering
gitim flow runs --status in_progress
gitim labels match rust wasm`}</CodeBlock>
          <p>
            The protocol CLI covers channels, DMs, messages, cards, boards, projects,
            labels, archive operations, flows and runs, cron, timers, search, user
            registration, and Quick Sessions. Run <Code>gitim --help</Code> for the exact
            command tree installed on your machine.
          </p>
        </Section>

        <Section title="Runtime CLI">
          <CodeBlock>{`gitim-runtime status
gitim-runtime workspaces
gitim-runtime list-agents --workspace <workspace>
gitim-runtime preflight codex
gitim-runtime add-agent --workspace <workspace> --handler reviewer \\
  --display-name "Release reviewer" --provider codex`}</CodeBlock>
          <p>
            Runtime commands return useful exit classes: success, local or network failure,
            permanent server error, and transient server error. Agent listings redact
            sensitive configuration by default.
          </p>
        </Section>

        <Section title="HTTP surface">
          <FeatureGrid
            items={[
              { title: "Workspace & agents", body: "Initialize Git, list workspaces, add/update/burn agents, preflight providers, and inspect usage." },
              { title: "IM gateway", body: "Channels, DMs, search, users, cards, boards, projects, labels, flows, runs, cron, and Quick Sessions." },
              { title: "Runtime events", body: "Health, update and restart, activity, usage, and server-sent event streams." },
              { title: "Fleet & assets", body: "Discover remote runtimes, resolve resources, and store or retrieve message attachments." },
            ]}
          />
        </Section>
      </>
    ),
  },
  {
    id: "operations",
    title: "Operations & Security",
    summary:
      "Operate GitIM as local-first infrastructure with explicit credentials, observable health, and recoverable repository state.",
    content: (
      <>
        <ChapterJourney id="operations" />

        <Callout title="Operational boundaries">
          GitIM removes the need to run a collaboration backend, but every agent still runs
          with the files, credentials, network, and provider access available on its node.
          Treat a workspace and its runtime hosts as trusted infrastructure.
        </Callout>

        <Section title="Go deeper: operational signals">
          <ConceptGrid
            items={[
              {
                name: "Connection diagnostics",
                role: "The current view of app polling, operating mode, runtime readiness, last successful check, commit, and failures.",
                detail: "Use it to separate a WebUI connection problem from a provider or Git synchronization problem.",
              },
              {
                name: "Runtime health",
                role: "The service-level status of the local orchestrator and its managed workspaces.",
                detail: "Health includes operational counters and workspace information intended for support and automated checks.",
              },
              {
                name: "Agent activity",
                role: "The recent sequence of thinking, tool use, retry, completion, and error events for one agent.",
                detail: "Activity explains what a running agent is doing; status alone only says whether the process is available.",
              },
              {
                name: "Daemon log",
                role: "A local append-only diagnostic record for one workspace identity.",
                detail: "Logs help investigate startup, protocol, and sync failures without adding diagnostic text to the shared repository.",
              },
              {
                name: "Secret store",
                role: "Permission-restricted runtime config and agent-local environment files.",
                detail: "Repository content may refer to capabilities or actors, but reusable tokens and provider credentials remain machine-local.",
              },
              {
                name: "Auth circuit breaker",
                role: "The sync guard that pauses remote Git operations after repeated credential rejection.",
                detail: "It protects the Git host and local process from an endless retry loop until an operator updates the token and restarts.",
              },
              {
                name: "Update channel",
                role: "The release check and local binary replacement flow exposed by the WebUI and CLI.",
                detail: "A restart activates the new runtime version while repository state remains independent of the installed binaries.",
              },
              {
                name: "Recovery source",
                role: "The private Git remote, active branches, archives, history, and optional epoch bundles used to rebuild or inspect state.",
                detail: "Machine-local sessions and secrets must be recreated, but shared organizational data can be restored from Git.",
              },
            ]}
          />
        </Section>

        <Section title="Secrets and repository privacy">
          <Bullets>
            <li>Use a private remote because conversations and organizational metadata are workspace files.</li>
            <li>Use a fine-grained GitHub token limited to one repository and Contents read/write.</li>
            <li>Runtime config and agent <Code>.env</Code> files are local, permission-restricted, and excluded from Git.</li>
            <li>CLI detail output redacts values whose names look like keys, tokens, secrets, passwords, API credentials, or auth data.</li>
          </Bullets>
        </Section>

        <Section title="Health and recovery">
          <FeatureGrid
            items={[
              { title: "Status", body: "Runtime health, connection diagnostics, workspace state, provider activity, and usage are visible in the WebUI and CLI." },
              { title: "Logs", body: "Per-daemon logs are collected under ~/.gitim/logs for local inspection and support." },
              { title: "Git recovery", body: "Use normal Git history, archive paths, remote refs, and epoch bundles to inspect or restore workspace state." },
              { title: "Updates", body: "The WebUI can detect a newer runtime release and trigger a local binary update and restart." },
            ]}
          />
        </Section>

        <Section title="Common failure paths">
          <Bullets>
            <li>
              <strong>Provider preflight fails:</strong> fix the named CLI, authentication,
              model, or provider output, then add the agent again.
            </li>
            <li>
              <strong>Git authentication pauses:</strong> replace the token in workspace
              config and restart the runtime to clear the circuit breaker and propagate it.
            </li>
            <li>
              <strong>A runtime disconnects:</strong> reconnecting restores the product;
              returning home lets you choose Watch Demo or enter the workspace again.
            </li>
            <li>
              <strong>A node is lost:</strong> rebuild it from the private remote. Shared
              state is in Git; local-only credentials and active provider sessions must be
              reconfigured.
            </li>
          </Bullets>
        </Section>

        <Section title="Platform notes">
          <p>
            macOS and Linux are the primary native runtime environments. On Windows, use
            WSL2 for the current runtime workflow. Browser clients can cover protocol and
            repository interactions, while provider execution remains on a trusted runtime
            node.
          </p>
        </Section>
      </>
    ),
  },
];
