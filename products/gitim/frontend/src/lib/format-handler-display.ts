/**
 * Shared resolution for the display-name render layer. The protocol identifier
 * is always the `handler`; `display_name` is a pure render-time enrichment
 * looked up from the directory (a `handler → display_name` map). Every handler
 * exposure point in the chat UI routes through these helpers so the fallback
 * semantics stay in one place.
 */

/**
 * Resolve a handler to its display name, or `undefined` when there's nothing
 * useful to show. Returns `undefined` both when the handler is unknown
 * (directory not loaded yet / departed / historical user) AND when the
 * display_name equals the handler — so callers render the bare `@handler`
 * instead of the redundant "alice @alice".
 */
export function resolveDisplayName(
  handler: string,
  directory: ReadonlyMap<string, string>,
): string | undefined {
  const name = directory.get(handler);
  if (!name || name === handler) return undefined;
  return name;
}

/**
 * Plain-string form for non-JSX contexts (aria-label, title, document.title).
 * "Alice Chen (@alice)" when a display_name is known, bare "@alice" otherwise.
 */
export function formatHandlerLabel(
  handler: string,
  directory: ReadonlyMap<string, string>,
): string {
  const name = resolveDisplayName(handler, directory);
  return name ? `${name} (@${handler})` : `@${handler}`;
}
