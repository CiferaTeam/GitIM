import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/theme/theme-toggle";
import { DemoStage } from "./demo-stage";
import { GitimLogo, LandingStory } from "./landing-story";
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

  function handleProtocolClick() {
    document
      .querySelector<HTMLElement>('[data-testid="landing-screen-messages"]')
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function handleBrandClick() {
    document
      .querySelector<HTMLElement>('[data-testid="landing-screen-intro"]')
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  return (
    <div
      className="min-h-screen flex flex-col bg-background text-foreground"
      data-testid="landing-page"
    >
      <header
        className="h-[4.5rem] shrink-0 border-b border-border bg-card/80 px-4 backdrop-blur-md sm:px-7"
        data-testid="landing-header"
      >
        <div className="mx-auto flex h-full max-w-[96rem] items-center justify-between gap-6">
          <button
            type="button"
            className="text-2xl text-foreground transition-colors hover:text-primary"
            onClick={handleBrandClick}
            data-testid="landing-brand"
          >
            <GitimLogo />
          </button>
          <div className="flex items-center gap-1 sm:gap-3">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="hidden sm:inline-flex"
              onClick={handleProtocolClick}
            >
              Protocol
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="hidden sm:inline-flex"
              onClick={() => navigate("/docs")}
            >
              Docs
            </Button>
            <Button
              type="button"
              size="sm"
              className="hidden sm:inline-flex"
              onClick={handleConnectRuntime}
              data-testid="landing-header-connect"
            >
              Connect runtime
            </Button>
            <ThemeToggle />
          </div>
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-hidden">
        <div className="h-[calc(100svh-4.5rem)] overflow-hidden [perspective:1800px]">
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
              className="relative h-full min-h-0 overflow-hidden [backface-visibility:hidden] [-webkit-backface-visibility:hidden] [grid-area:1/1]"
              aria-hidden={demoOpen}
              inert={demoOpen ? true : undefined}
              data-testid="landing-first-stage"
            >
              <LandingStory
                onConnectRuntime={handleConnectRuntime}
                onWatchDemo={handleWatchDemo}
              />
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
