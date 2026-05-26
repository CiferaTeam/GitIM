/**
 * Module-level event bus for workspace-switch coordination.
 *
 * Stores that own workspace-scoped state register a listener at module-load
 * time; whatever owns the workspace lifecycle (usePollLoop) calls
 * `emitWorkspaceSwitch()` once per actual switch instead of having to
 * enumerate every store by hand. That keeps the responsibility of "I am
 * workspace-scoped, here's how I reset" inside each store and makes adding a
 * new workspace-scoped store safe — you can't forget to wire the reset
 * because the wiring lives next to the store definition.
 *
 * Listeners are called in registration order. Order shouldn't matter for
 * correctness — each store resets its own slice independently — but if it
 * ever does, the controlling axis is import order of the store modules
 * (which is itself stable because all stores are imported from `App` and
 * `usePollLoop` at startup).
 */

type WorkspaceSwitchListener = () => void;

const listeners = new Set<WorkspaceSwitchListener>();

/**
 * Register a listener fired on every workspace switch. Returns an unsubscribe
 * function — useful in tests; production stores typically register once at
 * module load and never unregister.
 */
export function onWorkspaceSwitch(
  listener: WorkspaceSwitchListener,
): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Fire the workspace-switch event. Each listener runs in its own try/catch so
 * a failing listener can't prevent siblings from resetting — a partial reset
 * is strictly safer than a stuck workspace.
 */
export function emitWorkspaceSwitch(): void {
  for (const listener of listeners) {
    try {
      listener();
    } catch (err) {
      console.error("[workspaceLifecycle] listener threw:", err);
    }
  }
}

/**
 * Test-only: drop every registered listener. Production code should never
 * call this — stores rely on staying registered for the whole app lifetime.
 */
export function _resetWorkspaceLifecycleForTests(): void {
  listeners.clear();
}
