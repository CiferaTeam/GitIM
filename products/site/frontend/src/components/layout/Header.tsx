import { Button } from "@/components/ui/button"

const navLinks = [
  { label: "Features", href: "#features" },
  { label: "How It Works", href: "#how-it-works" },
]

export function Header() {
  return (
    <header className="fixed top-0 z-50 w-full border-b border-border bg-background/80 backdrop-blur-md">
      <div className="mx-auto flex h-16 max-w-[1200px] items-center justify-between px-6">
        <a href="/" className="flex items-center gap-2.5 font-display text-xl font-semibold">
          <img src="/logo.png" alt="GitIM" className="h-9 w-9" />
          <span>
            <span className="text-primary">Git</span>
            <span className="text-foreground">IM</span>
          </span>
        </a>

        <nav className="hidden md:flex items-center gap-8">
          {navLinks.map((link) => (
            <a
              key={link.href}
              href={link.href}
              className="text-sm text-text-secondary transition-colors hover:text-foreground"
            >
              {link.label}
            </a>
          ))}
        </nav>

        <Button size="sm" asChild>
          <a href="#access">Request Early Access</a>
        </Button>
      </div>
    </header>
  )
}
