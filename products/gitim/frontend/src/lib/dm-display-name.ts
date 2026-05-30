/**
 * The single peer handler of a 1:1 DM from `currentUser`'s point of view, or
 * `null` when `currentUser` isn't a participant or `name` isn't a well-formed
 * `a--b` pair. Self-DMs (`lewis--lewis`) resolve to the user themselves.
 */
export function dmPeerHandler(
  name: string,
  currentUser: string,
): string | null {
  const parts = name.split("--");
  if (parts.length !== 2) return null;
  if (parts[0] === currentUser) return parts[1];
  if (parts[1] === currentUser) return parts[0];
  return null;
}

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
