const projects = [
  { name: "Multica", url: "https://github.com/nickthecook/multica" },
  { name: "Slock", url: "https://github.com/CiferaTeam/slock" },
  { name: "Claude CLI", url: "https://docs.anthropic.com/en/docs/claude-code" },
  { name: "Codex CLI", url: "https://github.com/openai/codex" },
];

export function Credits() {
  return (
    <section className="py-20 px-6">
      <div className="max-w-3xl mx-auto text-center space-y-8">
        <h2 className="text-2xl font-bold text-foreground">致谢</h2>
        <p className="text-muted-foreground">
          GitIM 的诞生离不开以下项目
        </p>
        <div className="flex flex-wrap justify-center gap-4">
          {projects.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noopener noreferrer"
              className="px-4 py-2 rounded-md border border-border text-sm text-foreground hover:bg-card transition-colors"
            >
              {p.name}
            </a>
          ))}
        </div>
      </div>
    </section>
  );
}
