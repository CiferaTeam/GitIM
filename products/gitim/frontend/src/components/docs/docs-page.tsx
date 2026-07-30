import { useEffect, useRef } from "react";
import { useNavigate, useSearchParams } from "react-router";
import {
  ArrowLeft,
  ArrowRight,
  BookOpenText,
  Bot,
  ChevronRight,
  FileCode2,
  FolderGit,
  Globe2,
  KeyRound,
  LayoutDashboard,
  MessageSquare,
  Rocket,
  Server,
  ShieldCheck,
  Sparkles,
  SquareTerminal,
  Workflow,
  type LucideIcon,
} from "lucide-react";
import { DOC_GROUPS, DOC_SECTIONS, type DocSectionId } from "./docs-sections";

const ICONS: Record<DocSectionId, LucideIcon> = {
  quickstart: Rocket,
  workspaces: FolderGit,
  "github-token": KeyRound,
  agents: Bot,
  messaging: MessageSquare,
  "work-management": LayoutDashboard,
  automation: Workflow,
  "quick-sessions": Sparkles,
  protocol: FileCode2,
  runtime: Server,
  distributed: Globe2,
  "cli-api": SquareTerminal,
  operations: ShieldCheck,
};

function isDocSectionId(value: string | null): value is DocSectionId {
  return DOC_SECTIONS.some((section) => section.id === value);
}

export function DocsPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const contentRef = useRef<HTMLElement>(null);
  const requestedId = searchParams.get("tab");
  const activeId = isDocSectionId(requestedId) ? requestedId : "quickstart";
  const activeIndex = DOC_SECTIONS.findIndex((section) => section.id === activeId);
  const activeSection = DOC_SECTIONS[activeIndex];

  function selectChapter(id: DocSectionId) {
    setSearchParams({ tab: id });
  }

  function leaveDocs() {
    navigate("/");
  }

  useEffect(() => {
    const scrollOptions: ScrollToOptions = { top: 0, behavior: "auto" };
    if (typeof contentRef.current?.scrollTo === "function") {
      contentRef.current.scrollTo(scrollOptions);
    }
    if (typeof window.scrollTo === "function") {
      window.scrollTo(scrollOptions);
    }
  }, [activeId]);

  return (
    <div
      data-testid="docs-page"
      className="flex h-full min-h-0 bg-background text-foreground"
    >
      <aside className="hidden w-72 shrink-0 flex-col border-r border-border bg-card/45 lg:flex">
        <div className="border-b border-border/70 px-5 py-4">
          <button
            type="button"
            onClick={leaveDocs}
            className="flex items-center gap-2 text-sm text-text-secondary transition-colors hover:text-foreground"
          >
            <ArrowLeft className="size-4" />
            Back to GitIM
          </button>
        </div>

        <div className="border-b border-border/70 px-5 py-5">
          <div className="flex items-center gap-2 text-primary">
            <BookOpenText className="size-5" />
            <span className="text-sm font-semibold uppercase tracking-[0.16em]">
              Documentation
            </span>
          </div>
          <p className="mt-2 text-sm leading-relaxed text-text-muted">
            From first workspace to the protocol underneath it.
          </p>
        </div>

        <nav aria-label="Documentation chapters" className="flex-1 overflow-y-auto px-3 py-4">
          {DOC_GROUPS.map((group) => (
            <div
              key={group.id}
              data-testid={`docs-group-${group.id}`}
              className="mb-5 last:mb-0"
            >
              <p className="mb-1.5 px-3 text-[11px] font-semibold uppercase tracking-[0.14em] text-text-muted">
                {group.title}
              </p>
              <div className="space-y-0.5">
                {group.sectionIds.map((id) => {
                  const section = DOC_SECTIONS.find((item) => item.id === id);
                  if (!section) return null;
                  const Icon = ICONS[id];
                  const selected = id === activeId;
                  return (
                    <button
                      key={id}
                      type="button"
                      data-testid={`docs-nav-${id}`}
                      aria-current={selected ? "page" : undefined}
                      onClick={() => selectChapter(id)}
                      className={[
                        "group flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-left text-sm transition-colors",
                        selected
                          ? "bg-primary/12 font-medium text-foreground"
                          : "text-text-secondary hover:bg-surface/60 hover:text-foreground",
                      ].join(" ")}
                    >
                      <Icon
                        className={[
                          "size-4 shrink-0",
                          selected ? "text-primary" : "text-text-muted group-hover:text-text-secondary",
                        ].join(" ")}
                      />
                      <span className="min-w-0 flex-1">{section.title}</span>
                      <ChevronRight
                        className={[
                          "size-3.5 shrink-0 text-primary transition-opacity",
                          selected ? "opacity-100" : "opacity-0",
                        ].join(" ")}
                      />
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="border-b border-border bg-card/55 px-4 py-3 lg:hidden">
          <div className="mb-2 flex items-center justify-between">
            <button
              type="button"
              onClick={leaveDocs}
              className="flex items-center gap-1.5 text-sm text-text-secondary"
            >
              <ArrowLeft className="size-4" />
              Back
            </button>
            <span className="text-xs font-semibold uppercase tracking-[0.14em] text-primary">
              Docs
            </span>
          </div>
          <label className="sr-only" htmlFor="docs-chapter">
            Documentation chapter
          </label>
          <select
            id="docs-chapter"
            value={activeId}
            onChange={(event) => selectChapter(event.target.value as DocSectionId)}
            className="h-10 w-full rounded-md border border-border bg-background px-3 text-base text-foreground outline-none focus:border-primary"
          >
            {DOC_GROUPS.map((group) => (
              <optgroup key={group.id} label={group.title}>
                {group.sectionIds.map((id) => {
                  const section = DOC_SECTIONS.find((item) => item.id === id);
                  return section ? (
                    <option key={id} value={id}>
                      {section.title}
                    </option>
                  ) : null;
                })}
              </optgroup>
            ))}
          </select>
        </div>

        <main ref={contentRef} className="min-h-0 flex-1 overflow-y-auto">
          <article className="mx-auto w-full max-w-4xl px-5 py-8 sm:px-8 sm:py-12 lg:px-12 lg:py-14">
            <header className="mb-9 border-b border-border pb-7">
              <p className="mb-3 text-xs font-semibold uppercase tracking-[0.16em] text-primary">
                {DOC_GROUPS.find((group) => group.sectionIds.includes(activeId))?.title}
              </p>
              <h1
                data-testid="docs-heading"
                className="text-3xl font-bold tracking-tight text-foreground sm:text-4xl"
              >
                {activeSection.title}
              </h1>
              <p className="mt-3 max-w-3xl text-base leading-7 text-text-secondary sm:text-lg">
                {activeSection.summary}
              </p>
            </header>

            <div className="space-y-10">{activeSection.content}</div>

            <footer className="mt-14 grid gap-3 border-t border-border pt-6 sm:grid-cols-2">
              {activeIndex > 0 ? (
                <button
                  type="button"
                  onClick={() => selectChapter(DOC_SECTIONS[activeIndex - 1].id)}
                  className="group rounded-lg border border-border bg-card/40 p-4 text-left transition-colors hover:border-primary/45 hover:bg-card"
                >
                  <span className="flex items-center gap-1.5 text-xs uppercase tracking-[0.12em] text-text-muted">
                    <ArrowLeft className="size-3.5" />
                    Previous
                  </span>
                  <span className="mt-2 block text-base font-medium text-foreground">
                    {DOC_SECTIONS[activeIndex - 1].title}
                  </span>
                </button>
              ) : (
                <div />
              )}
              {activeIndex < DOC_SECTIONS.length - 1 ? (
                <button
                  type="button"
                  onClick={() => selectChapter(DOC_SECTIONS[activeIndex + 1].id)}
                  className="group rounded-lg border border-border bg-card/40 p-4 text-right transition-colors hover:border-primary/45 hover:bg-card"
                >
                  <span className="flex items-center justify-end gap-1.5 text-xs uppercase tracking-[0.12em] text-text-muted">
                    Next
                    <ArrowRight className="size-3.5" />
                  </span>
                  <span className="mt-2 block text-base font-medium text-foreground">
                    {DOC_SECTIONS[activeIndex + 1].title}
                  </span>
                </button>
              ) : null}
            </footer>
          </article>
        </main>
      </div>
    </div>
  );
}
