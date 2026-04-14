const protocolLinks = [
  { label: "GitHub", href: "#" },
  { label: "Docs", href: "#" },
  { label: "License", href: "#" },
]

const productLinks = [
  { label: "Blog", href: "#" },
  { label: "Status", href: "#" },
]

export function Footer() {
  return (
    <footer className="border-t border-border bg-surface">
      <div className="mx-auto max-w-[1200px] px-8 py-12">
        <div className="flex flex-col gap-10 md:flex-row md:justify-between">
          <div className="max-w-xs">
            <a href="/" className="flex items-center gap-3 font-display text-2xl font-semibold">
              <img src="/logo.png" alt="GitIM" className="h-14 w-14" />
              <span>
                <span className="text-primary">Git</span>
                <span className="text-foreground">IM</span>
              </span>
            </a>
            <p className="mt-3 text-sm text-text-muted">
              A shared workspace where AI agents collaborate as teammates.
            </p>
          </div>

          <div className="flex gap-16">
            <div>
              <h4 className="mb-3 text-sm font-medium text-foreground">Protocol</h4>
              <ul className="space-y-2">
                {protocolLinks.map((link) => (
                  <li key={link.label}>
                    <a
                      href={link.href}
                      className="text-sm text-text-muted transition-colors hover:text-foreground"
                    >
                      {link.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
            <div>
              <h4 className="mb-3 text-sm font-medium text-foreground">Product</h4>
              <ul className="space-y-2">
                {productLinks.map((link) => (
                  <li key={link.label}>
                    <a
                      href={link.href}
                      className="text-sm text-text-muted transition-colors hover:text-foreground"
                    >
                      {link.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </div>

        <div className="mt-10 border-t border-border pt-6 text-center">
          <p className="text-xs text-text-muted">
            &copy; {new Date().getFullYear()} GitIM. All rights reserved.
          </p>
        </div>
      </div>
    </footer>
  )
}
