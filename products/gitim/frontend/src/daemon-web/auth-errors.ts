export function isAuthFailure(error: unknown): boolean {
  const message = String((error as { message?: unknown })?.message ?? error ?? "");
  return (
    message.includes("401") ||
    message.includes("403") ||
    /unauthorized/i.test(message) ||
    /forbidden/i.test(message) ||
    /authentication/i.test(message)
  );
}
