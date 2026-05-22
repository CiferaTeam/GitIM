export function formatDmDisplayName(name: string, currentUser: string): string {
  const parts = name.split("--");
  if (parts.length !== 2) return name;

  const [a, b] = parts;
  if (a === currentUser && b === currentUser) return `${currentUser} (me)`;
  if (a === b) return a;
  if (a === currentUser) return b;
  if (b === currentUser) return a;
  return `${a} ↔ ${b}`;
}
