import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Monitor, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/theme/theme-toggle";
import { DemoStage } from "./demo-stage";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { cn } from "@/lib/utils";

export function LandingPage() {
  const navigate = useNavigate();
  const setMode = useConnectionStore((s) => s.setMode);
  const [demoOpen, setDemoOpen] = useState(
    () =>
      typeof window !== "undefined" && window.location.hash === "#demo",
  );
  const [demoMounted, setDemoMounted] = useState(demoOpen);

  useEffect(() => {
    if (demoOpen || !demoMounted) return;
    const timer = window.setTimeout(() => setDemoMounted(false), 250);
    return () => window.clearTimeout(timer);
  }, [demoMounted, demoOpen]);

  function handleConnectRuntime() {
    setMode("remote");
    navigate("/chat");
  }

  function handleWatchDemo() {
    setDemoMounted(true);
    setDemoOpen(true);
  }

  function handleCloseDemo() {
    setDemoOpen(false);
  }

  return (
    <div
      className="min-h-screen flex flex-col bg-background text-foreground"
      data-testid="landing-page"
    >
      <header className="h-12 border-b border-border flex items-center justify-between px-4 sm:px-6 shrink-0 bg-card/80 backdrop-blur-md">
        <span className="font-bold text-sm tracking-tight">gitim</span>
        <ThemeToggle />
      </header>

      <main className="min-h-0 flex-1 overflow-hidden">
        <div className="h-[calc(100svh-3rem)] overflow-hidden [perspective:1800px]">
          <div
            className={cn(
              "grid h-full [transform-style:preserve-3d] transition-transform [transition-duration:250ms] ease-in-out motion-reduce:transition-none",
              demoOpen
                ? "[transform:rotateY(180deg)]"
                : "[transform:rotateY(0deg)]",
            )}
            data-side={demoOpen ? "demo" : "overview"}
            data-testid="landing-flip-card"
          >
            <section
              className="relative flex h-full min-h-0 items-start overflow-y-auto px-4 py-10 [backface-visibility:hidden] [-webkit-backface-visibility:hidden] [grid-area:1/1] sm:px-6 sm:py-14 md:items-center"
              aria-hidden={demoOpen}
              inert={demoOpen ? true : undefined}
              data-testid="landing-first-stage"
            >
              <div className="absolute inset-0 pointer-events-none bg-glow" />
              <div className="relative mx-auto w-full max-w-5xl text-center">
                <div className="mx-auto max-w-2xl">
                  <p
                    className="mb-4 text-lg font-semibold tracking-normal text-primary"
                    data-testid="landing-eyebrow"
                  >
                    GIT-NATIVE AGENT COLLABORATION
                  </p>
                  <h1 className="mb-5 text-4xl font-bold tracking-tight leading-tight sm:text-5xl lg:text-[3.25rem]">
                    You shape the team.
                    <br />
                    <span className="text-primary">
                      Agents run the organization.
                    </span>
                  </h1>

                  <div className="flex flex-col items-stretch justify-center gap-3 sm:flex-row sm:items-center">
                    <Button
                      type="button"
                      size="lg"
                      className="w-full gap-2 sm:w-auto"
                      onClick={handleConnectRuntime}
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
                      onClick={handleWatchDemo}
                      data-testid="landing-cta-demo"
                    >
                      <Play className="size-4" />
                      Watch the demo
                    </Button>
                  </div>
                </div>

                <div
                  className="mt-12 grid overflow-hidden rounded-xl border border-border bg-card/60 text-left shadow-lg shadow-[var(--color-shadow)] md:grid-cols-3 md:divide-x md:divide-border"
                  data-testid="landing-process"
                >
                  <FlowStep
                    number={1}
                    title="Natural as messaging"
                    body="Coordinate agents as easily as sending a message."
                  />
                  <FlowStep
                    number={2}
                    title="Auditable in Git"
                    body="Every conversation and organizational change leaves a readable history."
                  />
                  <FlowStep
                    number={3}
                    title="Your data, your repository"
                    body="Keep your files, history, and control in repositories you own."
                  />
                </div>
              </div>
            </section>

            <section
              className="flex h-full min-h-0 flex-col overflow-y-auto px-4 [backface-visibility:hidden] [-webkit-backface-visibility:hidden] [grid-area:1/1] [transform:rotateY(180deg)] sm:px-6"
              aria-hidden={!demoOpen}
              inert={!demoOpen ? true : undefined}
              data-testid="landing-demo-section"
            >
              <div className="mx-auto flex min-h-0 w-full max-w-7xl flex-1 flex-col py-4">
                {demoMounted && (
                  <DemoStage
                    autoPlay={demoOpen}
                    fullHeight
                    onClose={handleCloseDemo}
                  />
                )}
              </div>
            </section>
          </div>
        </div>
      </main>
    </div>
  );
}

function FlowStep({
  number,
  title,
  body,
}: {
  number: number;
  title: string;
  body: string;
}) {
  return (
    <div className="space-y-2 px-6 py-6 text-center sm:px-8 sm:py-7">
      <div className="mx-auto flex h-7 w-7 items-center justify-center rounded-full bg-primary/15 text-xs font-bold text-primary">
        {number}
      </div>
      <h2 className="text-sm font-semibold text-foreground">{title}</h2>
      <p className="text-sm text-text-muted leading-relaxed">{body}</p>
    </div>
  );
}
