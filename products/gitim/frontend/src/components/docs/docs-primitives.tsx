import type { ReactNode } from "react";
import {
  ArrowRight,
  ChevronDown,
  GitCommitHorizontal,
  Route,
  type LucideIcon,
} from "lucide-react";
import { Link } from "react-router";

export function Section({
  title,
  eyebrow,
  children,
}: {
  title: string;
  eyebrow?: string;
  children: ReactNode;
}) {
  return (
    <section className="space-y-4">
      {eyebrow ? (
        <p className="text-xs font-semibold uppercase tracking-[0.14em] text-primary">
          {eyebrow}
        </p>
      ) : null}
      <h2 className="text-xl font-semibold tracking-tight text-foreground sm:text-2xl">
        {title}
      </h2>
      <div className="space-y-4 text-base leading-7 text-text-secondary">{children}</div>
    </section>
  );
}

export function Steps({
  items,
}: {
  items: Array<{ title: string; children: ReactNode }>;
}) {
  return (
    <div className="space-y-2">
      {items.map((item, index) => (
        <div key={item.title} className="flex gap-4">
          <div className="flex flex-col items-center">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-full border border-primary/30 bg-primary/10 text-sm font-bold text-primary">
              {index + 1}
            </span>
            {index < items.length - 1 ? (
              <span className="mt-2 min-h-8 w-px flex-1 bg-border" />
            ) : null}
          </div>
          <div className="pb-7">
            <h3 className="mb-1 text-base font-semibold text-foreground">{item.title}</h3>
            <div className="space-y-2 text-base leading-7 text-text-secondary">
              {item.children}
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

export function Callout({
  title,
  children,
  tone = "blue",
}: {
  title: string;
  children: ReactNode;
  tone?: "blue" | "neutral";
}) {
  return (
    <aside
      className={[
        "rounded-lg border px-5 py-4",
        tone === "blue"
          ? "border-primary/25 bg-primary/7"
          : "border-border bg-card/55",
      ].join(" ")}
    >
      <p className="mb-1 text-sm font-semibold text-foreground">{title}</p>
      <div className="text-base leading-7 text-text-secondary">{children}</div>
    </aside>
  );
}

export function Code({ children }: { children: ReactNode }) {
  return (
    <code className="rounded border border-border bg-surface px-1.5 py-0.5 font-mono text-[0.86em] text-foreground">
      {children}
    </code>
  );
}

export function CodeBlock({ children }: { children: string }) {
  return (
    <pre className="overflow-x-auto rounded-lg border border-border bg-[#111216] p-4 text-sm leading-6 text-[#d8dee9]">
      <code>{children}</code>
    </pre>
  );
}

export function FeatureGrid({
  items,
}: {
  items: Array<{ title: string; body: ReactNode }>;
}) {
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {items.map((item) => (
        <div key={item.title} className="rounded-lg border border-border bg-card/45 p-4">
          <h3 className="text-base font-semibold text-foreground">{item.title}</h3>
          <div className="mt-1.5 text-sm leading-6 text-text-secondary">{item.body}</div>
        </div>
      ))}
    </div>
  );
}

export function GuideFlow({
  title,
  caption,
  steps,
}: {
  title: string;
  caption: string;
  steps: Array<{
    icon: LucideIcon;
    title: string;
    body: string;
    meta?: string;
  }>;
}) {
  return (
    <figure
      data-testid="docs-concept-flow"
      className="overflow-hidden rounded-xl border border-border bg-card/45"
    >
      <figcaption className="border-b border-border/80 px-5 py-5 sm:px-6">
        <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.14em] text-primary">
          <Route className="size-4" />
          Big picture
        </div>
        <h2 className="mt-2 text-xl font-semibold tracking-tight text-foreground sm:text-2xl">
          {title}
        </h2>
        <p className="mt-2 max-w-3xl text-base leading-7 text-text-secondary">
          {caption}
        </p>
      </figcaption>

      <div className="grid gap-3 p-4 sm:grid-cols-2 sm:p-5">
        {steps.map((step, index) => {
          const Icon = step.icon;
          return (
            <article
              key={step.title}
              data-testid="docs-flow-step"
              className={[
                "min-w-0 rounded-lg border border-border/80 bg-background/65 p-4",
                steps.length % 2 === 1 && index === steps.length - 1
                  ? "sm:col-span-2"
                  : "",
              ].join(" ")}
            >
              <div className="flex items-start justify-between gap-4">
                <div className="flex min-w-0 items-start gap-3">
                  <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-primary/25 bg-primary/10 text-primary">
                    <Icon className="size-[18px]" />
                  </span>
                  <span className="min-w-0">
                    {step.meta ? (
                      <span className="block font-mono text-[11px] uppercase tracking-[0.1em] text-text-muted">
                        {step.meta}
                      </span>
                    ) : null}
                    <span className="mt-0.5 block text-base font-semibold text-foreground">
                      {step.title}
                    </span>
                    <span className="mt-1 block text-sm leading-6 text-text-secondary">
                      {step.body}
                    </span>
                  </span>
                </div>
                <span className="font-mono text-xs font-semibold text-text-faint">
                  {String(index + 1).padStart(2, "0")}
                </span>
              </div>
            </article>
          );
        })}
      </div>
    </figure>
  );
}

export function WorkedExample({
  id,
  title,
  intro,
  steps,
  artifact,
}: {
  id: string;
  title: string;
  intro: ReactNode;
  steps: Array<{ label: string; body: ReactNode }>;
  artifact: ReactNode;
}) {
  return (
    <section
      data-testid={`docs-example-${id}`}
      className="overflow-hidden rounded-xl border border-border bg-card/35"
    >
      <div className="grid md:grid-cols-[0.72fr_1.28fr]">
        <div className="border-b border-border/80 p-5 sm:p-6 md:border-r md:border-b-0">
          <p className="text-xs font-semibold uppercase tracking-[0.14em] text-primary">
            Worked example
          </p>
          <h2 className="mt-2 text-xl font-semibold tracking-tight text-foreground sm:text-2xl">
            {title}
          </h2>
          <div className="mt-3 text-base leading-7 text-text-secondary">{intro}</div>
        </div>

        <ol className="divide-y divide-border/70 px-5 sm:px-6">
          {steps.map((step, index) => (
            <li key={step.label} className="flex gap-4 py-4 first:pt-5 last:pb-5">
              <span className="flex size-7 shrink-0 items-center justify-center rounded-full border border-primary/25 bg-primary/10 text-xs font-bold text-primary">
                {index + 1}
              </span>
              <span className="min-w-0">
                <span className="block text-sm font-semibold text-foreground">
                  {step.label}
                </span>
                <span className="mt-1 block text-sm leading-6 text-text-secondary">
                  {step.body}
                </span>
              </span>
            </li>
          ))}
        </ol>
      </div>

      <div
        data-testid="docs-recorded-artifact"
        className="flex gap-3 border-t border-primary/20 bg-primary/7 px-5 py-4 sm:px-6"
      >
        <GitCommitHorizontal className="mt-0.5 size-4 shrink-0 text-primary" />
        <div className="min-w-0">
          <p className="text-sm font-semibold text-foreground">What GitIM records</p>
          <div className="mt-1 text-sm leading-6 text-text-secondary">{artifact}</div>
        </div>
      </div>
    </section>
  );
}

export function ConceptGrid({
  items,
}: {
  items: Array<{ name: string; role: ReactNode; detail?: ReactNode }>;
}) {
  return (
    <details
      data-testid="docs-concepts"
      className="group overflow-hidden rounded-xl border border-border bg-card/35"
    >
      <summary className="flex cursor-pointer list-none items-center justify-between gap-5 px-5 py-4 marker:content-none sm:px-6">
        <span className="min-w-0">
          <span className="block text-base font-semibold text-foreground">
            Explore the component model
          </span>
          <span className="mt-1 block truncate text-sm text-text-muted">
            {items
              .slice(0, 4)
              .map((item) => item.name)
              .join(" · ")}
            {items.length > 4 ? ` · +${items.length - 4} more` : ""}
          </span>
        </span>
        <span className="flex shrink-0 items-center gap-2 text-xs font-semibold uppercase tracking-[0.1em] text-text-muted">
          {items.length} concepts
          <ChevronDown className="size-4 transition-transform group-open:rotate-180" />
        </span>
      </summary>

      <div className="border-t border-border/80 px-3 py-2 sm:px-4">
        {items.map((item) => (
          <details
            key={item.name}
            data-testid="docs-concept"
            className="group/item border-b border-border/70 last:border-b-0"
          >
            <summary className="flex cursor-pointer list-none items-start justify-between gap-4 rounded-md px-2 py-3.5 marker:content-none hover:bg-surface/55">
              <span className="min-w-0">
                <span className="block text-sm font-semibold text-foreground">
                  {item.name}
                </span>
                <span className="mt-1 block text-sm leading-6 text-text-secondary">
                  {item.role}
                </span>
              </span>
              <ChevronDown className="mt-1 size-4 shrink-0 text-text-muted transition-transform group-open/item:rotate-180" />
            </summary>
            {item.detail ? (
              <div className="mx-2 mb-3 rounded-md border-l-2 border-primary/35 bg-background/55 px-4 py-3 text-sm leading-6 text-text-muted">
                {item.detail}
              </div>
            ) : null}
          </details>
        ))}
      </div>
    </details>
  );
}

export function ChapterLinks({
  items,
}: {
  items: Array<{ id: string; title: string; body: ReactNode; to: string }>;
}) {
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {items.map((item) => (
        <Link
          key={item.id}
          to={item.to}
          data-testid={`docs-next-${item.id}`}
          className="group rounded-lg border border-border bg-card/45 p-4 transition-colors hover:border-primary/45 hover:bg-surface/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
        >
          <span className="flex items-center justify-between gap-3">
            <span className="text-base font-semibold text-foreground">{item.title}</span>
            <ArrowRight className="size-4 shrink-0 text-text-muted transition-colors group-hover:text-primary" />
          </span>
          <span className="mt-1.5 block text-sm leading-6 text-text-secondary">
            {item.body}
          </span>
        </Link>
      ))}
    </div>
  );
}

export function Bullets({ children }: { children: ReactNode }) {
  return <ul className="list-disc space-y-2 pl-5">{children}</ul>;
}

export function Screenshot({ src, caption }: { src: string; caption: string }) {
  return (
    <figure className="overflow-hidden rounded-lg border border-border bg-card/40">
      <img src={src} alt={caption} className="block h-auto w-full" loading="lazy" />
      <figcaption className="border-t border-border px-4 py-2.5 text-sm text-text-muted">
        {caption}
      </figcaption>
    </figure>
  );
}
