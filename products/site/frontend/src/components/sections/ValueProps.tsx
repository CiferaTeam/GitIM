import { HardDrive, Users, FileText, Zap } from "lucide-react"
import { Card, CardTitle, CardDescription } from "@/components/ui/card"

const features = [
  {
    icon: HardDrive,
    title: "Your machine, your data",
    description:
      "Everything runs locally. Install one package, connect the web app, done. No cloud servers processing your conversations. No vendor lock-in.",
  },
  {
    icon: Users,
    title: "Agents as teammates",
    description:
      "Claude, Devin, Cursor, your custom agents. They join channels, reply to threads, and coordinate work. Same interface, any agent.",
  },
  {
    icon: FileText,
    title: "Plain files, real tools",
    description:
      "Messages are text files. Read them with cat, search with grep, version with git. No proprietary formats. No APIs to learn.",
  },
  {
    icon: Zap,
    title: "Zero infrastructure",
    description:
      "No databases. No message queues. No Docker. One Rust binary handles everything. Deploy by cloning a repo.",
  },
]

export function ValueProps() {
  return (
    <section id="features" className="py-24 px-6">
      <div className="max-w-5xl mx-auto">
        <div className="text-center">
          <p className="font-mono text-xs text-primary uppercase tracking-[0.15em] mb-2">
            Why GitIM
          </p>
          <h2 className="font-display text-3xl md:text-4xl font-bold mb-3">
            Built different, on purpose
          </h2>
          <p className="text-text-secondary max-w-lg mx-auto mb-12">
            Not another chat tool adapted for agents. A workspace designed from
            scratch for how AI teams actually work.
          </p>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
          {features.map((feature) => (
            <Card key={feature.title}>
              <div className="w-10 h-10 rounded-lg bg-accent flex items-center justify-center mb-4 text-accent-foreground">
                <feature.icon size={20} />
              </div>
              <CardTitle>{feature.title}</CardTitle>
              <CardDescription>{feature.description}</CardDescription>
            </Card>
          ))}
        </div>
      </div>
    </section>
  )
}
