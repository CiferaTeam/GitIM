import { useState } from "react";
import { Check, FolderInput } from "lucide-react";
import type { Project } from "../../lib/types";
import {
  validateProjectIntroduction,
  validateProjectName,
  validateProjectSlug,
} from "../../lib/client";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "../ui/popover";
import { Textarea } from "../ui/textarea";

interface CreateProjectDialogProps {
  open: boolean;
  onOpenChange(open: boolean): void;
  onCreate(
    slug: string,
    displayName: string,
    introduction: string,
  ): Promise<string | null>;
}

export function CreateProjectDialog({
  open,
  onOpenChange,
  onCreate,
}: CreateProjectDialogProps) {
  const [slug, setSlug] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [introduction, setIntroduction] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  function reset() {
    setSlug("");
    setDisplayName("");
    setIntroduction("");
    setError("");
    setSubmitting(false);
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function submit() {
    const normalizedSlug = slug.trim().toLowerCase();
    const validation = validateProjectSlug(normalizedSlug);
    if (validation) {
      setError(validation);
      return;
    }
    const nameValidation = validateProjectName(displayName);
    if (nameValidation) {
      setError(nameValidation);
      return;
    }
    const introductionValidation = validateProjectIntroduction(introduction);
    if (introductionValidation) {
      setError(introductionValidation);
      return;
    }

    setSubmitting(true);
    setError("");
    const createError = await onCreate(
      normalizedSlug,
      displayName.trim(),
      introduction.trim(),
    );
    if (createError) {
      setError(createError);
      setSubmitting(false);
      return;
    }
    reset();
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New project</DialogTitle>
          <DialogDescription>
            Create a Git-backed group for related channels.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <div className="grid gap-1.5">
            <label htmlFor="project-slug" className="text-sm font-medium">
              Slug
            </label>
            <Input
              id="project-slug"
              value={slug}
              onChange={(event) => setSlug(event.target.value)}
              placeholder="e.g. product-launch"
              autoFocus
            />
            <p className="text-[11px] text-muted-foreground">
              Lowercase letters, numbers, and hyphens. Max 32 chars.
            </p>
          </div>
          <div className="grid gap-1.5">
            <label htmlFor="project-name" className="text-sm font-medium">
              Name
            </label>
            <Input
              id="project-name"
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
              placeholder="e.g. Product Launch"
              maxLength={64}
            />
            <p className="text-[11px] text-muted-foreground">
              Maximum 64 UTF-8 bytes.
            </p>
          </div>
          <div className="grid gap-1.5">
            <label
              htmlFor="project-introduction"
              className="text-sm font-medium"
            >
              Introduction
            </label>
            <Textarea
              id="project-introduction"
              value={introduction}
              onChange={(event) => setIntroduction(event.target.value)}
              placeholder="What channels belong in this project?"
              maxLength={500}
            />
            <p className="text-[11px] text-muted-foreground">
              Maximum 500 UTF-8 bytes.
            </p>
          </div>
          {error && <p className="text-sm text-destructive">{error}</p>}
          <DialogFooter>
            <Button
              type="submit"
              disabled={
                submitting ||
                !slug.trim() ||
                !displayName.trim() ||
                !introduction.trim()
              }
              data-testid="create-project-submit"
            >
              {submitting ? "Creating…" : "Create project"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

interface ChannelProjectMenuProps {
  channel: string;
  projects: Project[];
  currentProject: string | null;
  busy: boolean;
  onAssign(project: string | null): Promise<void>;
}

export function ChannelProjectMenu({
  channel,
  projects,
  currentProject,
  busy,
  onAssign,
}: ChannelProjectMenuProps) {
  const [open, setOpen] = useState(false);

  function select(project: string | null) {
    if (busy) return;
    setOpen(false);
    if (project !== currentProject) void onAssign(project);
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label={`Move #${channel} to project`}
          title={`Move #${channel} to project`}
          disabled={busy}
          data-testid={`channel-project-trigger-${channel}`}
          className="mr-1 text-text-faint opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100"
        >
          <FolderInput className="size-3" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-56 p-1">
        <p className="px-2 py-1 text-[11px] font-medium text-text-muted">
          Move #{channel}
        </p>
        <ProjectOption
          active={currentProject === null}
          disabled={busy}
          testId="channel-project-option-unassigned"
          onSelect={() => select(null)}
        >
          No project
        </ProjectOption>
        {projects.map((project) => (
          <ProjectOption
            key={project.slug}
            active={currentProject === project.slug}
            disabled={busy}
            testId={`channel-project-option-${project.slug}`}
            onSelect={() => select(project.slug)}
          >
            {project.meta.display_name}
          </ProjectOption>
        ))}
        {projects.length === 0 && (
          <p className="px-2 py-1.5 text-xs text-text-muted">
            Create a project first.
          </p>
        )}
      </PopoverContent>
    </Popover>
  );
}

function ProjectOption({
  active,
  disabled,
  children,
  testId,
  onSelect,
}: {
  active: boolean;
  disabled: boolean;
  children: React.ReactNode;
  testId: string;
  onSelect(): void;
}) {
  return (
    <button
      type="button"
      className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
      data-testid={testId}
      disabled={disabled}
      onClick={onSelect}
    >
      <Check
        className={[
          "size-3 shrink-0",
          active ? "opacity-100" : "opacity-0",
        ].join(" ")}
      />
      <span className="truncate">{children}</span>
    </button>
  );
}
