import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { Monitor, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/theme/theme-toggle";
import { DemoStage } from "./demo-stage";
import { useConnectionStore } from "@/hooks/use-connection-store";

export function LandingPage() {
  const navigate = useNavigate();
  const setMode = useConnectionStore((s) => s.setMode);
  // Text-first opening: the demo stays collapsed until explicitly requested.
  // #demo deep-links (and anyone preferring the old always-on layout) open it
  // right away.
  const [demoOpen, setDemoOpen] = useState(
    () =>
      typeof window !== "undefined" && window.location.hash === "#demo",
  );
  const demoSectionRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!demoOpen) return;
    // Optional-call: jsdom does not implement scrollIntoView.
    demoSectionRef.current?.scrollIntoView?.({
      behavior: "smooth",
      block: "start",
    });
  }, [demoOpen]);

  function handleConnectRuntime() {
    setMode("remote");
    navigate("/chat");
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

      <main className="flex-1">
        <section className="relative overflow-hidden px-4 sm:px-6 py-10 sm:py-14 lg:py-16">
          <div className="absolute inset-0 bg-glow pointer-events-none" />
          <div className="relative max-w-5xl mx-auto text-center">
            <p className="text-lg sm:text-xl font-bold tracking-tight text-primary mb-4">
              gitim
            </p>
            <h1 className="text-3xl sm:text-4xl lg:text-[2.75rem] font-bold tracking-tight mb-4 leading-tight">
              You shape the team.
              <br />
              <span className="text-primary">Agents run the organization.</span>
            </h1>
            <p className="text-base sm:text-lg text-text-secondary max-w-2xl mx-auto mb-6 leading-relaxed">
              Describe the outcome. A coordinator uses GitIM&apos;s CLI to add
              agents, assign cards, run flows, and leave every organizational
              change in Git.
            </p>

            <div className="flex flex-col sm:flex-row items-center justify-center gap-3">
              <Button
                type="button"
                size="lg"
                className="w-full sm:w-auto gap-2"
                onClick={handleConnectRuntime}
                data-testid="landing-cta-connect"
              >
                <Monitor className="size-4" />
                Connect your runtime
              </Button>
              {!demoOpen && (
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  className="w-full sm:w-auto gap-2"
                  onClick={() => setDemoOpen(true)}
                  data-testid="landing-cta-demo"
                >
                  <Play className="size-4" />
                  Watch the demo
                </Button>
              )}
            </div>
          </div>
        </section>

        {demoOpen && (
          <section
            ref={demoSectionRef}
            className="px-4 sm:px-6 pb-6 scroll-mt-12 min-h-[calc(100vh-3rem)] flex flex-col"
            data-testid="landing-demo-section"
          >
            <div className="max-w-7xl w-full mx-auto flex-1 flex flex-col min-h-0 py-4">
              <DemoStage
                autoPlay
                fullHeight
                onClose={() => setDemoOpen(false)}
              />
            </div>
          </section>
        )}

        <section className="border-t border-border px-4 sm:px-6 py-12 bg-card/30">
          <div className="max-w-4xl mx-auto grid grid-cols-1 md:grid-cols-3 gap-8 text-center">
            <FlowStep
              number={1}
              title="Natural-language intent"
              body="You describe the organizational outcome in plain language."
            />
            <FlowStep
              number={2}
              title="Agent organization changes"
              body="A coordinator translates intent into CLI actions and Git commits."
            />
            <FlowStep
              number={3}
              title="Live Git-backed file tree"
              body="Every change is a file you can read, diff, and keep in your own repo."
            />
          </div>
        </section>
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
    <div className="space-y-2">
      <div className="w-7 h-7 rounded-full bg-primary/15 text-primary flex items-center justify-center text-xs font-bold mx-auto">
        {number}
      </div>
      <h2 className="text-sm font-semibold text-foreground">{title}</h2>
      <p className="text-sm text-text-muted leading-relaxed">{body}</p>
    </div>
  );
}
