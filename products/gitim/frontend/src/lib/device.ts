const UUID_KEY = "gitim:uuid";

export function getUUID(): string {
  let id = localStorage.getItem(UUID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(UUID_KEY, id);
  }
  return id;
}
