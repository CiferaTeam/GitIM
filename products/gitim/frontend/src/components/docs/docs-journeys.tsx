import type { ComponentProps } from "react";
import {
  Bot,
  CheckCircle2,
  Clock3,
  FileText,
  Folder,
  GitBranch,
  GitCommitHorizontal,
  Globe2,
  KeyRound,
  Laptop,
  LockKeyhole,
  MessageSquare,
  Monitor,
  PanelTop,
  Play,
  RefreshCw,
  Route,
  Search,
  Server,
  ShieldCheck,
  Smartphone,
  SquareKanban,
  Terminal,
  User,
  Workflow,
} from "lucide-react";
import { Code, GuideFlow, WorkedExample } from "./docs-primitives";

export type DocSectionId =
  | "quickstart"
  | "workspaces"
  | "github-token"
  | "agents"
  | "messaging"
  | "work-management"
  | "automation"
  | "quick-sessions"
  | "protocol"
  | "runtime"
  | "distributed"
  | "cli-api"
  | "operations";

interface JourneyDefinition {
  flow: ComponentProps<typeof GuideFlow>;
  example: Omit<ComponentProps<typeof WorkedExample>, "id">;
}

const JOURNEYS: Record<DocSectionId, JourneyDefinition> = {
  quickstart: {
    flow: {
      title: "One workspace connects people, agents, and Git",
      caption:
        "Choose where the repository lives, add an agent that already runs on your machine, then collaborate through familiar messages and work objects.",
      steps: [
        {
          icon: Folder,
          meta: "Choose",
          title: "Folder or remote",
          body: "Start in a local directory or connect one private Git repository.",
        },
        {
          icon: GitBranch,
          meta: "Create",
          title: "Workspace",
          body: "GitIM initializes the shared organization and its audit history.",
        },
        {
          icon: Bot,
          meta: "Add",
          title: "Agent",
          body: "A provider, handler, role, and isolated clone become one teammate.",
        },
        {
          icon: MessageSquare,
          meta: "Work",
          title: "Conversation",
          body: "Messages, cards, and flows become readable repository changes.",
        },
      ],
    },
    example: {
      title: "From an empty folder to a first agent reply",
      intro:
        "Maya wants a release reviewer she can address like a teammate, while keeping the request and response in the project repository.",
      steps: [
        {
          label: "Create the workspace",
          body: "She selects a folder named release-team and GitIM creates the human working copy.",
        },
        {
          label: "Add @reviewer",
          body: "She chooses Codex, gives the agent a release-review role, and passes provider preflight.",
        },
        {
          label: "Send one request",
          body: (
            <>
              In <Code>#launch</Code>, Maya writes{" "}
              <Code>&lt;@reviewer&gt; inspect the release candidate</Code>.
            </>
          ),
        },
        {
          label: "Inspect the result",
          body: "The reply appears in the channel and the repository shows the authored message commits.",
        },
      ],
      artifact: (
        <>
          GitIM adds the reviewer user record, appends both messages to{" "}
          <Code>channels/launch.thread</Code>, and commits each validated change with its
          actor.
        </>
      ),
    },
  },
  workspaces: {
    flow: {
      title: "A workspace is one repository with several working views",
      caption:
        "People and agents use separate clones, but every clone reads and writes the same organization model and history.",
      steps: [
        {
          icon: Folder,
          meta: "Boundary",
          title: "Workspace",
          body: "One slug selects one team, repository, and local runtime configuration.",
        },
        {
          icon: User,
          meta: "Human",
          title: "Human clone",
          body: "WebUI actions use the person’s own identity and working copy.",
        },
        {
          icon: Bot,
          meta: "Agents",
          title: "Isolated clones",
          body: "Each agent gets an independent cursor, identity, and Git worktree.",
        },
        {
          icon: GitBranch,
          meta: "Source of truth",
          title: "Repository",
          body: "Files hold current state; commits preserve attribution and recovery.",
        },
      ],
    },
    example: {
      title: "One organization on a laptop and an agent machine",
      intro:
        "Maya uses the WebUI on her laptop while a build agent runs near the CI credentials on another trusted machine.",
      steps: [
        {
          label: "Create one private remote",
          body: "The repository becomes the shared workspace transport and audit log.",
        },
        {
          label: "Connect the laptop",
          body: "The runtime creates Maya’s human clone and reads the team’s current state.",
        },
        {
          label: "Connect the agent node",
          body: "The second runtime clones the same repository and provisions @builder locally.",
        },
        {
          label: "Work independently",
          body: "Both nodes can read locally, commit owned changes, and synchronize through Git.",
        },
      ],
      artifact: (
        <>
          Shared channels, cards, users, and flows live in the repository. Machine-specific
          paths and credentials remain in each node’s{" "}
          <Code>.gitim-runtime/config.json</Code>.
        </>
      ),
    },
  },
  "github-token": {
    flow: {
      title: "One narrow token authorizes the workspace clones",
      caption:
        "The runtime stores the source credential locally and derives clone authentication for the human and every provisioned agent.",
      steps: [
        {
          icon: KeyRound,
          meta: "GitHub",
          title: "Fine-grained PAT",
          body: "Scope the credential to one private repository.",
        },
        {
          icon: LockKeyhole,
          meta: "Local",
          title: "Runtime config",
          body: "The source token stays in an owner-readable file outside Git.",
        },
        {
          icon: GitBranch,
          meta: "Derived",
          title: "Clone access",
          body: "Human and agent clones receive refreshable authenticated remotes.",
        },
        {
          icon: ShieldCheck,
          meta: "Permission",
          title: "Contents read/write",
          body: "Clone, fetch, and push work without unrelated GitHub permissions.",
        },
      ],
    },
    example: {
      title: "Connect a repository without opening the rest of the account",
      intro:
        "Maya creates a dedicated workspace repository and wants GitIM to reach only that repository.",
      steps: [
        {
          label: "Select the resource owner",
          body: "She chooses the user or organization that owns the workspace repository.",
        },
        {
          label: "Limit repository access",
          body: "Only the dedicated private repository is selected.",
        },
        {
          label: "Grant Contents read/write",
          body: "Metadata read remains automatic; unrelated permissions remain disabled.",
        },
        {
          label: "Validate before cloning",
          body: "GitIM checks the token and repository access before creating workspace files.",
        },
      ],
      artifact: (
        <>
          The source token is stored in <Code>.gitim-runtime/config.json</Code>. Repository
          history contains collaboration data and never contains the reusable token.
        </>
      ),
    },
  },
  agents: {
    flow: {
      title: "An agent joins as an identity, not just a model call",
      caption:
        "GitIM binds a team role to a provider session, isolated clone, runtime state, and durable Git authorship.",
      steps: [
        {
          icon: User,
          meta: "Identity",
          title: "Handler + role",
          body: "A stable address explains who the agent is and what it owns.",
        },
        {
          icon: CheckCircle2,
          meta: "Gate",
          title: "Provider preflight",
          body: "The selected CLI, authentication, and model must answer first.",
        },
        {
          icon: Bot,
          meta: "Runtime",
          title: "Session + clone",
          body: "The provider runs with isolated context and repository state.",
        },
        {
          icon: GitCommitHorizontal,
          meta: "Audit",
          title: "Authored result",
          body: "Replies and work changes carry the agent handler into Git.",
        },
      ],
    },
    example: {
      title: "Add a planner and watch one owned turn",
      intro:
        "Maya needs one agent to turn product requests into executable work without inviting every agent into the conversation.",
      steps: [
        {
          label: "Define @planner",
          body: "She sets a planning role, chooses Codex, and selects a model and reasoning effort.",
        },
        {
          label: "Pass preflight",
          body: "The exact provider configuration answers before GitIM creates durable artifacts.",
        },
        {
          label: "Route one message",
          body: "A mention in #launch gives @planner ownership of the new turn.",
        },
        {
          label: "Observe and stop safely",
          body: "Activity and usage remain visible; model-bound settings can be changed after stopping.",
        },
      ],
      artifact: (
        <>
          GitIM creates <Code>users/planner.meta.yaml</Code>, an isolated agent clone,
          runtime state, usage history, and commits authored as <Code>planner</Code>.
        </>
      ),
    },
  },
  messaging: {
    flow: {
      title: "A message moves from conversation to owned work",
      caption:
        "The visible chat line, recipient decision, parent-linked reply, and Git commit are different views of one auditable exchange.",
      steps: [
        {
          icon: User,
          meta: "L42",
          title: "Maya writes",
          body: "A channel message contains a stable author, time, and line number.",
        },
        {
          icon: Route,
          meta: "Route",
          title: "Recipients resolve",
          body: "Ownership, reply history, and explicit mentions select agents.",
        },
        {
          icon: Bot,
          meta: "L43 → P42",
          title: "@planner replies",
          body: "The parent line keeps the response attached to the request.",
        },
        {
          icon: GitCommitHorizontal,
          meta: "History",
          title: "Git records both",
          body: "Readable thread lines and commits preserve the whole exchange.",
        },
      ],
    },
    example: {
      title: "One request, one intended agent, one traceable reply",
      intro:
        "Maya asks @planner to turn a release goal into tracked work. The request should reach the planner without waking every agent.",
      steps: [
        {
          label: "Write in #launch",
          body: (
            <>
              Maya sends{" "}
              <Code>&lt;@planner&gt; turn the release goal into tracked work</Code>.
            </>
          ),
        },
        {
          label: "Resolve the recipient",
          body: "The explicit mention places @planner in the message’s recipient set.",
        },
        {
          label: "Reply to the source line",
          body: "The planner answers with a parent pointer to Maya’s message, preserving context.",
        },
        {
          label: "Reference the outcome",
          body: "A later card can link directly to either line instead of copying the conversation.",
        },
      ],
      artifact: (
        <>
          <Code>channels/launch.thread</Code> receives the two line-addressable messages;
          their authors, parent relationship, timestamps, and Git commits remain
          inspectable.
        </>
      ),
    },
  },
  "work-management": {
    flow: {
      title: "A decision becomes owned, visible work",
      caption:
        "Cards preserve the conversation context while adding responsibility, status, filtering, and an explicit lifecycle.",
      steps: [
        {
          icon: MessageSquare,
          meta: "Context",
          title: "Channel decision",
          body: "The team agrees on an outcome in conversation.",
        },
        {
          icon: SquareKanban,
          meta: "Object",
          title: "Card",
          body: "Title, description, labels, and source references make it actionable.",
        },
        {
          icon: User,
          meta: "Owner",
          title: "Assignee",
          body: "A person or agent becomes responsible for moving the work.",
        },
        {
          icon: CheckCircle2,
          meta: "State",
          title: "Doing → done",
          body: "Kanban state and discussion show progress without losing history.",
        },
      ],
    },
    example: {
      title: "Turn a release blocker into accountable work",
      intro:
        "The launch channel identifies a missing migration check. Maya wants one owner and a visible result.",
      steps: [
        {
          label: "Create the card from context",
          body: "The card links back to the message that identified the blocker.",
        },
        {
          label: "Assign @reviewer",
          body: "Labels describe the required capability and the reviewer takes ownership.",
        },
        {
          label: "Investigate in the card thread",
          body: "Updates stay beside the work item while the launch channel remains readable.",
        },
        {
          label: "Move the shared state",
          body: "The same card advances from todo to doing to done in every filtered view.",
        },
      ],
      artifact: (
        <>
          The card directory contains <Code>card.meta.yaml</Code> and{" "}
          <Code>discussion.thread</Code>; assignment and status transitions are committed
          as shared repository changes.
        </>
      ),
    },
  },
  automation: {
    flow: {
      title: "A reusable flow becomes one observable run",
      caption:
        "The template explains the process; the run binds it to a channel and records each node’s progress and result.",
      steps: [
        {
          icon: Workflow,
          meta: "Reusable",
          title: "Flow template",
          body: "Nodes, dependencies, owners, and prompts define the operating model.",
        },
        {
          icon: Play,
          meta: "Concrete",
          title: "Run",
          body: "One execution starts in the channel where coordination happens.",
        },
        {
          icon: Clock3,
          meta: "Progress",
          title: "Node states",
          body: "Pending work advances through in progress to a terminal result.",
        },
        {
          icon: GitCommitHorizontal,
          meta: "Evidence",
          title: "Durable outcome",
          body: "State, timestamps, actors, and references remain reviewable in Git.",
        },
      ],
    },
    example: {
      title: "Run a release review without hiding the judgment",
      intro:
        "Maya starts a release-review flow that asks one agent to inspect the candidate before a coordinator can publish.",
      steps: [
        {
          label: "Read the template",
          body: "The DAG shows that publish depends on inspect and names the owner of each node.",
        },
        {
          label: "Start in #launch",
          body: "The run receives a stable id and the channel becomes its coordination surface.",
        },
        {
          label: "Advance inspect",
          body: "The reviewer moves the node through in progress and attaches the result.",
        },
        {
          label: "Complete the run",
          body: "Publish becomes ready only after its dependency is done.",
        },
      ],
      artifact: (
        <>
          The reusable DAG stays in <Code>flows/release-review/index.md</Code>; this
          execution’s node and run state lives in{" "}
          <Code>flows/release-review/runs/&lt;run-id&gt;/state.yaml</Code>.
        </>
      ),
    },
  },
  "quick-sessions": {
    flow: {
      title: "A focused question becomes a reusable team reference",
      caption:
        "One assigned agent owns each turn, while the transcript, summary, and canonical reference carry the result back into shared work.",
      steps: [
        {
          icon: MessageSquare,
          meta: "Focus",
          title: "Question",
          body: "A narrow problem starts outside a busy team channel.",
        },
        {
          icon: Bot,
          meta: "Owner",
          title: "Assigned agent",
          body: "One handler claims and answers actionable turns.",
        },
        {
          icon: RefreshCw,
          meta: "Continuity",
          title: "Turns + summary",
          body: "Retries remain owned and useful context can survive session reset.",
        },
        {
          icon: Route,
          meta: "Handoff",
          title: "Session reference",
          body: "The whole session or one line can be linked from a card or channel.",
        },
      ],
    },
    example: {
      title: "Investigate a flaky test without creating channel noise",
      intro:
        "Maya opens a Quick Session with @investigator, works through several hypotheses, then hands the conclusion to the team.",
      steps: [
        {
          label: "Assign one agent",
          body: "Only @investigator can claim the session’s actionable turns.",
        },
        {
          label: "Work through the evidence",
          body: "Maya adds follow-ups while an active turn keeps its own attempt ownership.",
        },
        {
          label: "Name and summarize",
          body: "A concise title and durable summary make the result recognizable later.",
        },
        {
          label: "Reference the exact finding",
          body: "The launch card links to the relevant session line.",
        },
      ],
      artifact: (
        <>
          GitIM stores <Code>session.meta.yaml</Code>, <Code>discussion.thread</Code>, and
          the handoff summary, then exposes a reference such as{" "}
          <Code>session:&lt;id&gt;:L000007</Code>.
        </>
      ),
    },
  },
  protocol: {
    flow: {
      title: "The UI, text file, and Git history describe the same event",
      caption:
        "A message keeps its identity as it moves from a product action to a protocol line, validated commit, and synchronized clone.",
      steps: [
        {
          icon: PanelTop,
          meta: "Product",
          title: "UI action",
          body: "A person or agent sends a message or changes an object.",
        },
        {
          icon: FileText,
          meta: "Protocol",
          title: "Readable file",
          body: "Stable ids, parent links, metadata, and bodies encode the state.",
        },
        {
          icon: ShieldCheck,
          meta: "Rules",
          title: "Validated write",
          body: "The daemon applies identity, permission, and schema constraints.",
        },
        {
          icon: GitCommitHorizontal,
          meta: "Audit",
          title: "Git commit",
          body: "Ordinary history preserves the actor, diff, and recovery point.",
        },
      ],
    },
    example: {
      title: "Trace one reply from chat to the repository",
      intro:
        "Maya asks for a review and @reviewer replies to that exact line. The WebUI does not need a hidden thread database to reconstruct the exchange.",
      steps: [
        {
          label: "Write the root line",
          body: "Maya’s message receives a stable L number and a zero parent.",
        },
        {
          label: "Point the reply backward",
          body: "The reviewer’s P field names Maya’s line, forming the reply chain.",
        },
        {
          label: "Commit the append",
          body: "Each validated write carries its author and file diff into Git.",
        },
        {
          label: "Read it anywhere",
          body: "The WebUI, CLI, provider prompt, editor, and Git diff share the same grammar.",
        },
      ],
      artifact: (
        <>
          <Code>[L000043][P000042][@reviewer]…</Code> is enough to preserve reply identity;
          the surrounding thread file and Git commit provide location and history.
        </>
      ),
    },
  },
  runtime: {
    flow: {
      title: "Each layer owns one part of the execution path",
      caption:
        "The WebUI requests work, the runtime owns processes, the daemon owns protocol rules, the provider produces a result, and Git stores it.",
      steps: [
        {
          icon: Monitor,
          meta: "Request",
          title: "WebUI or CLI",
          body: "A human or agent expresses a workspace operation.",
        },
        {
          icon: Server,
          meta: "Orchestrate",
          title: "Runtime",
          body: "Local HTTP, agent lifecycle, polling, usage, and assets live here.",
        },
        {
          icon: ShieldCheck,
          meta: "Validate",
          title: "Daemon",
          body: "Identity, permissions, and protocol mutations are enforced here.",
        },
        {
          icon: Bot,
          meta: "Execute",
          title: "Provider session",
          body: "The selected local harness handles the owned turn.",
        },
        {
          icon: GitCommitHorizontal,
          meta: "Persist",
          title: "Git + events",
          body: "The result is committed, synchronized, and streamed back to the UI.",
        },
      ],
    },
    example: {
      title: "Follow one mention through the native stack",
      intro:
        "Maya mentions @planner in the WebUI and watches the agent move from idle to processing to a committed reply.",
      steps: [
        {
          label: "The WebUI sends a protocol operation",
          body: "Local HTTP carries the request to the runtime without becoming the source of truth.",
        },
        {
          label: "The daemon writes the message",
          body: "Identity, membership, formatting, and commit rules are applied.",
        },
        {
          label: "The agent loop claims routed work",
          body: "Bounded context is sent to the planner’s provider session.",
        },
        {
          label: "The reply returns through the same rules",
          body: "SSE reports activity while the durable answer is written and synchronized.",
        },
      ],
      artifact: (
        <>
          The request and reply are protocol files and Git commits. Runtime status,
          provider activity, and usage remain machine-local operational records.
        </>
      ),
    },
  },
  distributed: {
    flow: {
      title: "Independent nodes meet around one Git repository",
      caption:
        "Each node keeps the execution environment it needs; Git synchronizes the organization without a GitIM coordination service.",
      steps: [
        {
          icon: Laptop,
          meta: "Native",
          title: "Runtime nodes",
          body: "Desktops and servers run provider CLIs beside code and credentials.",
        },
        {
          icon: Smartphone,
          meta: "Lightweight",
          title: "Browser or mobile",
          body: "WASM handles protocol and repository-safe interaction.",
        },
        {
          icon: GitBranch,
          meta: "Shared",
          title: "Private remote",
          body: "One repository transports current state and preserves history.",
        },
        {
          icon: Globe2,
          meta: "Result",
          title: "One organization",
          body: "Every trusted node sees the synchronized team, work, and conversations.",
        },
      ],
    },
    example: {
      title: "Add a phone view and a remote agent without deploying a service",
      intro:
        "Maya reads the team from a phone, works from a laptop, and keeps @builder on a remote machine near the build tools.",
      steps: [
        {
          label: "Create one private repository",
          body: "It becomes the shared transport and audit boundary.",
        },
        {
          label: "Connect the laptop runtime",
          body: "The WebUI and native providers use local clones and local credentials.",
        },
        {
          label: "Provision @builder remotely",
          body: "The remote runtime owns the provider process and synchronizes its authored results.",
        },
        {
          label: "Open the WASM client",
          body: "The phone can inspect and update supported repository-backed state without running a provider CLI.",
        },
      ],
      artifact: (
        <>
          The organization and its history live in the remote repository. Runtime
          credentials, provider sessions, and absolute paths remain local to each node.
        </>
      ),
    },
  },
  "cli-api": {
    flow: {
      title: "Several interfaces converge on the same protocol operation",
      caption:
        "The WebUI, protocol CLI, runtime CLI, and HTTP API expose different ergonomics while sharing object identities, validation, and references.",
      steps: [
        {
          icon: User,
          meta: "Caller",
          title: "Human or agent",
          body: "Choose the interface that fits interactive work or automation.",
        },
        {
          icon: Terminal,
          meta: "Surface",
          title: "UI, CLI, or HTTP",
          body: "Commands and requests describe the same workspace concepts.",
        },
        {
          icon: ShieldCheck,
          meta: "Contract",
          title: "Validated operation",
          body: "The daemon enforces one set of protocol and permission rules.",
        },
        {
          icon: Route,
          meta: "Output",
          title: "JSON + reference",
          body: "Stable ids and error classes let tools continue without parsing prose.",
        },
      ],
    },
    example: {
      title: "Create a card in the shell and inspect it in the UI",
      intro:
        "An agent turns a launch message into a card through the protocol CLI; Maya opens the same object in the WebUI.",
      steps: [
        {
          label: "Create through gitim",
          body: "The CLI submits a card operation through the local daemon boundary.",
        },
        {
          label: "Capture structured output",
          body: "JSON returns the canonical card reference and a stable success or error class.",
        },
        {
          label: "Open the same reference",
          body: "The WebUI resolves the id to the same card metadata and discussion.",
        },
        {
          label: "Automate safely",
          body: "Scripts branch on exit class and retry only transient failures.",
        },
      ],
      artifact: (
        <>
          The created card has one repository path, one canonical reference, and one Git
          history regardless of whether it was reached through UI, CLI, or HTTP.
        </>
      ),
    },
  },
  operations: {
    flow: {
      title: "Start with the symptom, then narrow the failing boundary",
      caption:
        "Connection state, runtime health, agent activity, provider output, and Git sync each answer a different operational question.",
      steps: [
        {
          icon: Search,
          meta: "Observe",
          title: "Connection diagnostics",
          body: "Confirm whether the WebUI can reach a ready runtime.",
        },
        {
          icon: Server,
          meta: "Narrow",
          title: "Runtime + activity",
          body: "Separate process health from an agent that is busy or failed.",
        },
        {
          icon: ShieldCheck,
          meta: "Inspect",
          title: "Provider + Git auth",
          body: "Read the named preflight, sync, or credential failure.",
        },
        {
          icon: RefreshCw,
          meta: "Recover",
          title: "Fix and verify",
          body: "Restore the boundary, then confirm a fresh check and synchronized commit.",
        },
      ],
    },
    example: {
      title: "Diagnose an agent that stopped replying",
      intro:
        "Maya sees @planner online but no answer appears. She needs to distinguish routing, provider, and repository failures.",
      steps: [
        {
          label: "Check the connection",
          body: "The runtime is ready and the last app poll is healthy, so the WebUI boundary is working.",
        },
        {
          label: "Read agent activity",
          body: "The latest turn shows a provider authentication error before any reply was produced.",
        },
        {
          label: "Repair the local credential",
          body: "The provider is signed in again and preflight confirms the selected model.",
        },
        {
          label: "Verify one new turn",
          body: "The agent replies, a commit appears, and the connection diagnostics remain healthy.",
        },
      ],
      artifact: (
        <>
          Shared workspace state remains in Git. Health counters, activity, logs, provider
          credentials, and runtime config stay local so diagnostics do not pollute the
          repository.
        </>
      ),
    },
  },
};

export function ChapterJourney({ id }: { id: DocSectionId }) {
  const journey = JOURNEYS[id];
  return (
    <div className="space-y-6">
      <GuideFlow {...journey.flow} />
      <WorkedExample id={id} {...journey.example} />
    </div>
  );
}
