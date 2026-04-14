import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"

export function AccessForm() {
  return (
    <section id="access" className="py-24 px-6">
      <div className="max-w-md mx-auto">
        <div className="bg-card border border-border rounded-xl p-10 md:p-12 relative overflow-hidden text-center">
          <div
            className="absolute top-0 left-1/2 -translate-x-1/2 w-48 h-px"
            style={{
              background:
                "linear-gradient(to right, transparent, #0EA5E9, transparent)",
            }}
          />

          <h2 className="font-display text-2xl font-bold mb-2">
            Request Early Access
          </h2>
          <p className="text-sm text-text-secondary mb-8">
            We're opening GitIM to a small group of builders. Enter your email
            and invite code to get started.
          </p>

          <form className="flex flex-col gap-3" onSubmit={(e) => e.preventDefault()}>
            <Input type="email" placeholder="your@email.com" aria-label="Email address" />
            <Input type="text" placeholder="Invite code" aria-label="Invite code" />
            <Button type="submit" className="w-full mt-1">
              Get Access
            </Button>
          </form>

          <p className="mt-4 font-mono text-xs text-text-muted">
            <span className="text-primary">12</span> of 50 spots remaining
          </p>
        </div>
      </div>
    </section>
  )
}
