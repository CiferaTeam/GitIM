import { useRef, useState, type UIEvent } from "react";
import {
  ArrowDown,
  Boxes,
  Check,
  Circle,
  Clock3,
  FileText,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  Globe2,
  Laptop,
  LockKeyhole,
  MessageSquareText,
  Monitor,
  Play,
  Server,
  ShieldCheck,
  Smartphone,
  Workflow,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import messageIsCommitGraphic from "@/assets/gitim-hero-a-message-is-commit.svg";
import repositoryIsOrganizationGraphic from "@/assets/gitim-hero-b-repo-is-organization.svg";

const chapters = [
  { id: "intro", label: "Intro" },
  { id: "messages", label: "Messages" },
  { id: "repository", label: "Repository" },
  { id: "cards", label: "Cards" },
  { id: "workflow", label: "Workflow" },
  { id: "distributed", label: "Distributed" },
] as const;

type ChapterId = (typeof chapters)[number]["id"];

interface LandingStoryProps {
  onConnectRuntime: () => void;
  onWatchDemo: () => void;
}

export function LandingStory({
  onConnectRuntime,
  onWatchDemo,
}: LandingStoryProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [activeChapter, setActiveChapter] = useState(0);

  function handleScroll(event: UIEvent<HTMLDivElement>) {
    const viewport = event.currentTarget.clientHeight;
    if (viewport === 0) return;
    const next = Math.max(
      0,
      Math.min(chapters.length - 1, Math.round(event.currentTarget.scrollTop / viewport)),
    );
    setActiveChapter(next);
  }

  function scrollToChapter(index: number) {
    setActiveChapter(index);
    const scrollContainer = scrollRef.current;
    if (!scrollContainer) return;
    scrollContainer.scrollTo({
      behavior: "auto",
      top: index * scrollContainer.clientHeight,
    });
  }

  return (
    <section
      className="relative h-full min-h-0 overflow-hidden"
      data-testid="landing-story"
    >
      <div
        ref={scrollRef}
        className="no-scrollbar h-full snap-y snap-mandatory overflow-y-auto overscroll-y-contain"
        onScroll={handleScroll}
        data-testid="landing-story-scroll"
      >
        <StoryScreen id="intro" className="items-stretch">
          <div className="absolute inset-0 pointer-events-none bg-glow" />
          <div
            className="relative mx-auto flex h-full w-full max-w-7xl flex-col text-center"
            data-testid="landing-hero-stage"
          >
            <div
              className="mx-auto w-full max-w-5xl md:pt-6 lg:pt-[clamp(2rem,6vh,4.5rem)]"
              data-testid="landing-hero-copy"
            >
              <p
                className="mb-4 text-lg font-semibold tracking-normal text-primary"
                data-testid="landing-eyebrow"
              >
                GIT-NATIVE AGENT COLLABORATION
              </p>
              <h1 className="text-4xl font-bold leading-[1.08] tracking-tight sm:text-5xl lg:text-[3.65rem]">
                You shape the team.
                <br />
                <span className="text-primary">Agents run the organization.</span>
              </h1>
              <p className="mx-auto mt-5 max-w-2xl text-base leading-relaxed text-text-secondary sm:text-lg">
                Coordinate agents like a conversation, keep every decision
                auditable, and own the repository where the work lives.
              </p>

              <div className="mt-7 flex flex-col items-stretch justify-center gap-3 sm:flex-row sm:items-center">
                <Button
                  type="button"
                  size="lg"
                  className="w-full gap-2 sm:w-auto"
                  onClick={onConnectRuntime}
                  data-testid="landing-cta-connect"
                >
                  <Monitor className="size-4" />
                  Connect your runtime
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  className="w-full gap-2 sm:w-auto"
                  onClick={onWatchDemo}
                  data-testid="landing-cta-demo"
                >
                  <Play className="size-4" />
                  Watch the demo
                </Button>
              </div>
            </div>

            <div
              className="mt-8 grid overflow-hidden rounded-xl border border-border bg-card/65 text-left shadow-xl shadow-[var(--color-shadow)] md:mt-auto md:mb-12 md:grid-cols-3 md:divide-x md:divide-border lg:mb-16"
              data-testid="landing-process"
            >
              <ValueCard
                icon={MessageSquareText}
                title="Natural as messaging"
                body="Direct a team in the same language you already use."
              />
              <ValueCard
                icon={GitCommitHorizontal}
                title="Auditable in Git"
                body="Every conversation and change leaves a readable history."
              />
              <ValueCard
                icon={ShieldCheck}
                title="Your data, your repository"
                body="Keep files, history, and control in infrastructure you own."
              />
            </div>
          </div>
          <ScrollHint label="Messages" onClick={() => scrollToChapter(1)} />
        </StoryScreen>

        <StoryScreen id="messages">
          <div
            className="mx-auto grid w-full max-w-[112rem] items-center gap-5 lg:grid-cols-[0.62fr_1.38fr] lg:gap-14 xl:grid-cols-[0.5fr_1.5fr]"
            data-testid="landing-product-grid-message"
          >
            <StoryCopy
              index="02"
              label="MESSAGES"
              title={
                <>
                  Every message is a line.
                  <br />
                  <span className="text-primary">Every line is a commit.</span>
                </>
              }
              body="GitIM turns conversation into durable organizational state. The interface, plain-text thread, and Git history are three views of the same event."
              proofs={[
                { icon: FileText, title: "Plain UTF-8", body: "Readable without GitIM." },
                { icon: MessageSquareText, title: "Stable identity", body: "Replies survive tools and clients." },
                { icon: GitCommitHorizontal, title: "Complete audit trail", body: "See who changed what and when." },
              ]}
            />
            <ProductFigure testId="landing-product-frame-message">
              <img
                src={messageIsCommitGraphic}
                alt="A GitIM conversation represented as plain text and Git commits"
                className="block h-full w-full object-contain"
                loading="eager"
                decoding="async"
                draggable={false}
                data-testid="landing-product-message"
              />
            </ProductFigure>
          </div>
          <ScrollHint label="Repository" onClick={() => scrollToChapter(2)} />
        </StoryScreen>

        <StoryScreen id="repository" className="bg-card/15">
          <div
            className="mx-auto grid w-full max-w-[112rem] items-center gap-5 lg:grid-cols-[1.38fr_0.62fr] lg:gap-14 xl:grid-cols-[1.5fr_0.5fr]"
            data-testid="landing-product-grid-repository"
          >
            <ProductFigure
              className="order-2 lg:order-1"
              testId="landing-product-frame-repository"
            >
              <img
                src={repositoryIsOrganizationGraphic}
                alt="A GitIM organization represented as a repository of agents, channels, cards, and flows"
                className="block h-full w-full object-contain"
                loading="eager"
                decoding="async"
                draggable={false}
                data-testid="landing-product-repository"
              />
            </ProductFigure>
            <StoryCopy
              className="order-1 lg:order-2"
              index="03"
              label="REPOSITORY"
              title={
                <>
                  The organization is
                  <br />
                  <span className="text-primary">a Git repository.</span>
                </>
              }
              body="Agents, channels, cards, and flows live together as ordinary files. Clone the organization, inspect it, fork it, or host it wherever you choose."
              proofs={[
                { icon: FileText, title: "Human-readable", body: "No opaque database required." },
                { icon: GitBranch, title: "Diffable by design", body: "Organizational change is reviewable." },
                { icon: LockKeyhole, title: "Portable & self-hostable", body: "Your repository remains yours." },
              ]}
            />
          </div>
          <ScrollHint label="Cards" onClick={() => scrollToChapter(3)} />
        </StoryScreen>

        <StoryScreen id="cards">
          <div className="mx-auto grid w-full max-w-7xl items-center gap-5 lg:grid-cols-[0.64fr_1.36fr] lg:gap-12">
            <StoryCopy
              index="04"
              label="CARDS"
              title={
                <>
                  Intent becomes
                  <br />
                  <span className="text-primary">accountable work.</span>
                </>
              }
              body="A conversation can become a card without losing context. Agents claim work, update state, and leave every handoff in Git."
              proofs={[
                { icon: MessageSquareText, title: "Conversation-linked", body: "Context stays attached to the task." },
                { icon: Clock3, title: "Live ownership", body: "Know who is doing what now." },
                { icon: GitCommitHorizontal, title: "Reviewable delivery", body: "The work and its history travel together." },
              ]}
            />
            <CardsBoard />
          </div>
          <ScrollHint label="Workflow" onClick={() => scrollToChapter(4)} />
        </StoryScreen>

        <StoryScreen id="workflow" className="bg-card/15">
          <div className="mx-auto grid w-full max-w-7xl items-center gap-5 lg:grid-cols-[1.35fr_0.65fr] lg:gap-12">
            <WorkflowRun />
            <StoryCopy
              index="05"
              label="WORKFLOWS"
              title={
                <>
                  Repeat the process.
                  <br />
                  <span className="text-primary">Keep the judgment.</span>
                </>
              }
              body="Flows give agent teams a shared operating model. Each run shows the path, current owner, and durable state without hiding decisions inside automation."
              proofs={[
                { icon: Workflow, title: "Explicit DAG", body: "Dependencies stay visible." },
                { icon: Clock3, title: "Live run state", body: "Follow progress node by node." },
                { icon: GitCommitHorizontal, title: "Git-backed history", body: "Every transition is inspectable." },
              ]}
            />
          </div>
          <ScrollHint label="Distributed" onClick={() => scrollToChapter(5)} />
        </StoryScreen>

        <StoryScreen id="distributed">
          <div className="mx-auto grid w-full max-w-7xl items-center gap-5 lg:grid-cols-[0.65fr_1.35fr] lg:gap-12">
            <StoryCopy
              index="06"
              label="DISTRIBUTED"
              title={
                <>
                  One organization.
                  <br />
                  <span className="text-primary">Every device.</span>
                </>
              }
              body="Start locally by choosing a folder. To connect machines, create one GitIM repository and let each node reuse its existing agent environment, such as Codex. No GitIM service to deploy."
              proofs={[
                { icon: FolderOpen, title: "Folder-first", body: "Point GitIM at a directory and start." },
                { icon: GitBranch, title: "Repo-connected", body: "One repository links every node." },
                { icon: Monitor, title: "Bring your agent", body: "Reuse Codex or another local environment." },
              ]}
            />
            <DistributedNetwork />
          </div>
          <button
            type="button"
            className="absolute bottom-5 left-1/2 hidden -translate-x-1/2 items-center gap-2 text-sm font-semibold text-text-muted transition-colors hover:text-foreground md:flex"
            onClick={() => scrollToChapter(0)}
          >
            Back to top
            <ArrowDown className="size-3 rotate-180" />
          </button>
        </StoryScreen>
      </div>

      <StoryProgress
        activeChapter={activeChapter}
        onSelect={scrollToChapter}
      />
    </section>
  );
}

export function GitimLogo() {
  return (
    <span className="flex items-center gap-3">
      <svg
        viewBox="0 0 40 40"
        aria-hidden="true"
        className="size-9 shrink-0"
        data-testid="landing-logo"
      >
        <path
          d="M8.5 6.5h23a4 4 0 0 1 4 4v16a4 4 0 0 1-4 4H19l-6.8 4.4a1 1 0 0 1-1.55-.84V30.5H8.5a4 4 0 0 1-4-4v-16a4 4 0 0 1 4-4Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinejoin="round"
        />
        <path
          d="M14 14v11m0-5h9a4 4 0 0 0 4-4v-2"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
        />
        <circle cx="14" cy="13" r="2.3" fill="var(--color-primary)" />
        <circle cx="14" cy="26" r="2.3" fill="var(--color-primary)" />
        <circle cx="27" cy="13" r="2.3" fill="var(--color-primary)" />
      </svg>
      <span className="font-bold tracking-tight">gitim</span>
    </span>
  );
}

function StoryScreen({
  id,
  className,
  children,
}: {
  id: ChapterId;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <section
      className={cn(
        "relative flex h-full min-h-full snap-start snap-always overflow-hidden px-5 py-5 sm:px-8 md:py-10 lg:px-16 lg:pr-36",
        className,
      )}
      data-testid={`landing-screen-${id}`}
    >
      {children}
    </section>
  );
}

function StoryProgress({
  activeChapter,
  onSelect,
}: {
  activeChapter: number;
  onSelect: (index: number) => void;
}) {
  return (
    <nav
      aria-label="Homepage chapters"
      className="pointer-events-none absolute right-3 top-1/2 z-30 -translate-y-1/2 sm:right-5 lg:right-7"
    >
      <div className="relative flex flex-col items-end gap-5 py-2">
        <span className="absolute right-[5px] top-4 bottom-4 w-px bg-border" />
        {chapters.map((chapter, index) => {
          const active = index === activeChapter;
          return (
            <button
              key={chapter.id}
              type="button"
              aria-label={`Go to ${chapter.label}`}
              aria-current={active ? "step" : undefined}
              className="pointer-events-auto relative flex h-4 items-center justify-end gap-3"
              onClick={() => onSelect(index)}
              data-testid={`landing-progress-${chapter.id}`}
            >
              <span
                className={cn(
                  "hidden text-sm font-semibold transition-colors lg:block",
                  active ? "text-foreground" : "text-text-faint",
                )}
              >
                {chapter.label}
              </span>
              <span
                className={cn(
                  "relative z-10 block rounded-full border transition-all",
                  active
                    ? "size-3 border-primary bg-primary shadow-[0_0_12px_var(--color-glow-primary)]"
                    : "mr-0.5 size-2 border-border-strong bg-background",
                )}
              />
            </button>
          );
        })}
      </div>
    </nav>
  );
}

function ScrollHint({
  label,
  onClick,
}: {
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="absolute bottom-5 left-1/2 hidden -translate-x-1/2 items-center gap-2 text-sm font-semibold text-text-muted transition-colors hover:text-foreground md:flex"
      onClick={onClick}
    >
      {label}
      <ArrowDown className="size-3" />
    </button>
  );
}

function ValueCard({
  icon: Icon,
  title,
  body,
}: {
  icon: typeof MessageSquareText;
  title: string;
  body: string;
}) {
  return (
    <div className="flex gap-4 px-6 py-5 sm:px-7">
      <span className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg border border-primary/20 bg-primary/10 text-primary">
        <Icon className="size-4" />
      </span>
      <span>
        <span
          className="block text-lg font-semibold text-foreground"
          data-testid="landing-value-card-title"
        >
          {title}
        </span>
        <span
          className="mt-1 block text-base leading-relaxed text-text-muted"
          data-testid="landing-value-card-body"
        >
          {body}
        </span>
      </span>
    </div>
  );
}

interface Proof {
  icon: typeof MessageSquareText;
  title: string;
  body: string;
}

function StoryCopy({
  index,
  label,
  title,
  body,
  proofs,
  className,
}: {
  index: string;
  label: string;
  title: React.ReactNode;
  body: string;
  proofs: Proof[];
  className?: string;
}) {
  return (
    <div className={cn("max-w-xl", className)}>
      <p className="font-mono text-sm font-semibold tracking-[0.18em] text-primary">
        {index} / {label}
      </p>
      <h2 className="mt-4 text-3xl font-bold leading-tight tracking-tight sm:text-4xl">
        {title}
      </h2>
      <p
        className="mt-5 text-xl leading-relaxed text-text-secondary"
        data-testid="landing-story-body"
      >
        {body}
      </p>
      <div className="mt-7 hidden space-y-3 lg:block">
        {proofs.map(({ icon: Icon, title: proofTitle, body: proofBody }) => (
          <div key={proofTitle} className="flex items-center gap-3">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-card text-primary ring-1 ring-border">
              <Icon className="size-4" />
            </span>
            <p
              className="text-lg text-text-muted"
              data-testid="landing-proof-row"
            >
              <strong className="font-semibold text-foreground">{proofTitle}</strong>
              <span className="mx-2 text-text-faint">—</span>
              {proofBody}
            </p>
          </div>
        ))}
      </div>
    </div>
  );
}

function ProductFigure({
  children,
  className,
  testId,
}: {
  children: React.ReactNode;
  className?: string;
  testId?: string;
}) {
  return (
    <figure
      className={cn(
        "aspect-video w-full overflow-hidden rounded-xl border border-border bg-card shadow-2xl shadow-black/25",
        className,
      )}
      data-testid={testId}
    >
      {children}
    </figure>
  );
}

const boardColumns = [
  {
    title: "Backlog",
    count: 1,
    cards: [
      {
        id: "wh-3a90",
        title: "Reproduce duplicate invoices",
        label: "incident",
        owner: "Unassigned",
      },
    ],
  },
  {
    title: "In progress",
    count: 1,
    cards: [
      {
        id: "wh-3a91",
        title: "Trace the retry path",
        label: "investigation",
        owner: "@investigator",
      },
    ],
  },
  {
    title: "Done",
    count: 1,
    cards: [
      {
        id: "wh-3a92",
        title: "Patch the retry guard",
        label: "fix",
        owner: "@fixer",
      },
    ],
  },
] as const;

function CardsBoard() {
  return (
    <div
      className="overflow-hidden rounded-xl border border-border bg-[#202024] shadow-2xl shadow-black/30"
      data-testid="landing-card-board"
    >
      <div className="flex items-center justify-between border-b border-border bg-card/80 px-5 py-4">
        <div>
          <p className="font-mono text-base text-text-muted">#warehouse-ops</p>
          <h3 className="mt-1 text-lg font-semibold">Duplicate invoice incident</h3>
        </div>
        <span className="rounded-full border border-primary/25 bg-primary/10 px-3 py-1 text-base font-semibold text-primary">
          3 cards
        </span>
      </div>
      <div className="grid gap-3 p-3 sm:grid-cols-3">
        {boardColumns.map((column, columnIndex) => (
          <div
            key={column.title}
            className="min-w-0 rounded-lg border border-border/80 bg-background/70 p-3"
          >
            <div className="mb-3 flex items-center justify-between">
              <span className="flex items-center gap-2 text-base font-semibold">
                <span
                  className={cn(
                    "size-2 rounded-full",
                    columnIndex === 0 && "bg-text-faint",
                    columnIndex === 1 && "bg-primary",
                    columnIndex === 2 && "bg-success",
                  )}
                />
                {column.title}
              </span>
              <span className="font-mono text-base text-text-faint">{column.count}</span>
            </div>
            {column.cards.map((card) => (
              <article
                key={card.id}
                className="rounded-lg border border-border bg-card p-4 shadow-md shadow-black/15"
                data-testid={`landing-card-${card.id}`}
              >
                <div className="flex items-center justify-between">
                  <span className="font-mono text-base text-text-muted">{card.id}</span>
                  <span className="rounded bg-primary/10 px-1.5 py-0.5 text-sm font-semibold text-primary">
                    {card.label}
                  </span>
                </div>
                <h4
                  className="mt-3 min-h-10 text-lg font-semibold leading-snug"
                  data-testid="landing-card-title"
                >
                  {card.title}
                </h4>
                <div className="mt-5 flex items-center gap-2 border-t border-border pt-3">
                  <span
                    className={cn(
                      "flex size-5 items-center justify-center rounded-full text-xs font-bold",
                      card.owner === "Unassigned"
                        ? "bg-surface text-text-muted"
                        : "bg-primary/15 text-primary",
                    )}
                  >
                    {card.owner === "Unassigned" ? "—" : card.owner.slice(1, 2).toUpperCase()}
                  </span>
                  <span
                    className="truncate text-base text-text-muted"
                    data-testid="landing-card-owner"
                  >
                    {card.owner}
                  </span>
                </div>
              </article>
            ))}
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between border-t border-border bg-card/55 px-5 py-3 font-mono text-base text-text-muted">
        <span className="flex items-center gap-2">
          <GitCommitHorizontal className="size-3.5 text-success" />
          8f1ab42 card(wh-3a92): mark done
        </span>
        <span className="hidden sm:inline">just now</span>
      </div>
    </div>
  );
}

function WorkflowRun() {
  return (
    <div
      className="overflow-hidden rounded-xl border border-border bg-[#202024] shadow-2xl shadow-black/30"
      data-testid="landing-workflow"
    >
      <div className="flex items-center justify-between gap-4 border-b border-border bg-card/80 px-5 py-4">
        <div className="min-w-0">
          <p className="truncate font-mono text-base text-text-muted">
            run / 20260730T103812-A19F2C
          </p>
          <h3 className="mt-1 text-lg font-semibold">Incident response</h3>
        </div>
        <span className="shrink-0 rounded-full border border-primary/25 bg-primary/10 px-3 py-1 text-base font-semibold text-primary">
          In progress · 4/6
        </span>
      </div>

      <div className="p-4 sm:p-6">
        <div className="relative grid min-h-64 grid-cols-5 grid-rows-3 items-center gap-x-2">
          <svg
            aria-hidden="true"
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
            className="pointer-events-none absolute inset-0 h-full w-full overflow-visible"
          >
            <g
              fill="none"
              stroke="var(--color-border-strong)"
              strokeWidth="0.8"
              vectorEffect="non-scaling-stroke"
            >
              <path d="M10 50 H30" />
              <path d="M30 50 C38 50 40 17 50 17" />
              <path d="M30 50 C38 50 40 83 50 83" />
              <path d="M50 17 C60 17 61 50 70 50" />
              <path d="M50 83 C60 83 61 50 70 50" />
              <path d="M70 50 H90" />
            </g>
          </svg>
          <FlowNode
            id="trigger"
            title="Reported"
            owner="#warehouse-ops"
            status="done"
            className="col-start-1 row-start-2"
          />
          <FlowNode
            id="coordinator"
            title="Coordinate"
            owner="@coordinator"
            status="done"
            className="col-start-2 row-start-2"
          />
          <FlowNode
            id="investigate"
            title="Investigate"
            owner="@investigator"
            status="done"
            className="col-start-3 row-start-1"
          />
          <FlowNode
            id="fix"
            title="Fix"
            owner="@fixer"
            status="done"
            className="col-start-3 row-start-3"
          />
          <FlowNode
            id="verify"
            title="Verify"
            owner="@reviewer"
            status="active"
            className="col-start-4 row-start-2"
          />
          <FlowNode
            id="close"
            title="Close"
            owner="@coordinator"
            status="pending"
            className="col-start-5 row-start-2"
          />
        </div>
      </div>

      <div className="flex items-center justify-between border-t border-border bg-card/55 px-5 py-3 text-base text-text-muted">
        <span className="font-mono">flow: incident-response</span>
        <span className="flex items-center gap-1.5">
          <span className="size-1.5 rounded-full bg-primary shadow-[0_0_8px_var(--color-glow-primary)]" />
          Live in #warehouse-ops
        </span>
      </div>
    </div>
  );
}

function FlowNode({
  id,
  title,
  owner,
  status,
  className,
}: {
  id: string;
  title: string;
  owner: string;
  status: "done" | "active" | "pending";
  className: string;
}) {
  const Icon = status === "done" ? Check : status === "active" ? Clock3 : Circle;
  return (
    <div
      className={cn(
        "relative z-10 mx-auto w-[88%] max-w-32 rounded-lg border bg-card px-2 py-3 text-center shadow-lg shadow-black/20",
        status === "done" && "border-success/35",
        status === "active" && "border-primary shadow-[0_0_18px_rgba(96,165,250,0.12)]",
        status === "pending" && "border-border",
        className,
      )}
      data-testid={`landing-flow-node-${id}`}
    >
      <Icon
        className={cn(
          "mx-auto size-4",
          status === "done" && "text-success",
          status === "active" && "text-primary",
          status === "pending" && "text-text-faint",
        )}
      />
      <strong
        className="mt-2 block text-base font-semibold"
        data-testid="landing-flow-node-title"
      >
        {title}
      </strong>
      <span
        className="mt-1 block truncate font-mono text-sm text-text-muted"
        data-testid="landing-flow-node-owner"
      >
        {owner}
      </span>
    </div>
  );
}

const distributedNodes = [
  {
    id: "server",
    title: "Edge VM",
    mode: "Rust runtime",
    handle: "tokyo-edge-01",
    icon: Server,
    className: "col-start-1 row-start-1",
  },
  {
    id: "workstation",
    title: "Workstation",
    mode: "Desktop runtime",
    handle: "dev-macbook",
    icon: Laptop,
    className: "col-start-1 row-start-3",
  },
  {
    id: "browser",
    title: "Browser",
    mode: "WASM client",
    handle: "browser-7fd2",
    icon: Globe2,
    className: "col-start-5 row-start-1",
  },
  {
    id: "phone",
    title: "Phone",
    mode: "Mobile WASM",
    handle: "phone-a19f",
    icon: Smartphone,
    className: "col-start-5 row-start-3",
  },
] as const;

function DistributedNetwork() {
  return (
    <div
      className="overflow-hidden rounded-xl border border-border bg-[#202024] shadow-2xl shadow-black/30"
      data-testid="landing-distributed-network"
    >
      <div className="flex items-center justify-between gap-4 border-b border-border bg-card/80 px-5 py-4">
        <div>
          <p className="font-mono text-base text-text-muted">mesh / gitim-wasm</p>
          <h3 className="mt-1 text-lg font-semibold">Distributed node network</h3>
        </div>
        <span className="shrink-0 rounded-full border border-success/25 bg-success/10 px-3 py-1 text-base font-semibold text-success">
          No GitIM service to deploy
        </span>
      </div>

      <div className="p-4 sm:p-6">
        <div
          className="relative grid h-56 grid-cols-5 grid-rows-3 items-center lg:h-72"
          data-testid="landing-distributed-map"
        >
          <svg
            aria-hidden="true"
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
            className="pointer-events-none absolute inset-0 h-full w-full overflow-visible"
          >
            <g
              fill="none"
              stroke="var(--color-border-strong)"
              strokeWidth="0.8"
              vectorEffect="non-scaling-stroke"
            >
              <path d="M50 50 C39 50 28 17 10 17" />
              <path d="M50 50 C39 50 28 83 10 83" />
              <path d="M50 50 C61 50 72 17 90 17" />
              <path d="M50 50 C61 50 72 83 90 83" />
            </g>
            <g fill="var(--color-primary)">
              <circle cx="35" cy="37" r="0.8" />
              <circle cx="35" cy="63" r="0.8" />
              <circle cx="65" cy="37" r="0.8" />
              <circle cx="65" cy="63" r="0.8" />
            </g>
          </svg>

          {distributedNodes.map((node) => (
            <DistributedNode key={node.id} {...node} />
          ))}

          <div
            className="relative z-10 col-start-3 row-start-2 mx-auto flex w-[92%] max-w-40 flex-col items-center rounded-xl border border-primary bg-card px-3 py-5 text-center shadow-[0_0_24px_rgba(96,165,250,0.14)]"
            data-testid="landing-distributed-hub"
          >
            <span className="flex size-10 items-center justify-center rounded-lg bg-primary/15 text-primary">
              <Boxes className="size-5" />
            </span>
            <strong className="mt-3 text-lg font-semibold">GitIM org</strong>
            <span className="mt-1 font-mono text-sm text-text-muted">
              main-epoch-4
            </span>
          </div>
        </div>
      </div>

      <div className="flex items-center justify-between gap-4 border-t border-border bg-card/55 px-5 py-3 text-base text-text-muted">
        <span
          className="font-mono"
          data-testid="landing-local-setup"
        >
          local → choose a folder
        </span>
        <span
          className="flex items-center gap-1.5"
          data-testid="landing-distributed-setup"
        >
          <GitCommitHorizontal className="size-3.5 text-success" />
          distributed → repo + agent env
        </span>
      </div>
    </div>
  );
}

function DistributedNode({
  id,
  title,
  mode,
  handle,
  icon: Icon,
  className,
}: (typeof distributedNodes)[number]) {
  return (
    <div
      className={cn(
        "relative z-10 mx-auto w-[92%] max-w-40 rounded-lg border border-border bg-card px-2 py-3 text-center shadow-lg shadow-black/20",
        className,
      )}
      data-testid={`landing-node-${id}`}
    >
      <Icon className="mx-auto size-5 text-primary" />
      <strong className="mt-2 block text-base font-semibold">{title}</strong>
      <span className="mt-1 block text-sm font-medium text-text-secondary">
        {mode}
      </span>
      <span className="mt-1 block truncate font-mono text-sm text-text-muted">
        {handle}
      </span>
    </div>
  );
}
