# Mobile WASM Workspaces Verification

- `cd products/gitim/frontend && npm test`
  - Passed: 14 test files, 87 tests.
- `cd products/gitim/frontend && npm run build`
  - Passed: `tsc -b && vite build`.
- `cd products/gitim/frontend && npm run lint`
  - Passed with no warnings.
- `cd products/gitim/frontend && npm run dev -- --host 127.0.0.1`
  - Started Vite for Playwright, then stopped it after e2e verification.
- `cd products/gitim/frontend && npm run test:e2e`
  - Passed: 19 tests across `e2e/sidebar-layout.spec.ts` and `e2e/mobile-layout.spec.ts`.
- `git diff --check`
  - Passed with no whitespace errors.

## Review Fix Verification

- Legacy `gitim-local-config` is now migrated from browser workspace list paths, and start-over/forget clears the legacy config so it does not reappear.
- 401/403 auth failures now demote the active browser workspace to reconnect-required state, clear only that workspace's session token, and preserve cached repo state.
- Drafts, pinned conversations, and known-agent sidebar state now use mode-aware workspace identity keys.
- Browser setup list exposes reset cache, forget workspace, and start-over actions before activation.
- Failed browser workspace activation no longer starts a poll loop against the previous active backend.
- Cached browser workspaces reject reconnect attempts that change the remote URL without a cache reset.
- Refresh restores the persisted active browser workspace when multiple session tokens are present.
- Reconnect rollback restores the previous session token for existing workspaces.
- Cached browser workspace activation that needs a token now opens cached data in a disconnected state without scheduling local sync polling.
- Stored active runtime slugs still initialize after the workspace list loads; the app depends on stable workspace identity instead of the full workspace array.
- Browser cache reset/start-over now awaits LightningFS wipe activation before reporting success.
- Browser reconnect now preserves the remote sync baseline when cached local commits are ahead, so auth-recovery sync pushes local commits instead of resetting them away.
- Failed activation of a newly created browser workspace now wipes its IndexedDB cache before deleting the registry entry.
- Browser workspace activation preserves user-provided workspace names while still filling the inferred handler.
- Browser poll no longer marks an unpushed local head as synced; sync baseline only advances after sync-owned push/fast-forward/merge paths.
- Browser sync conflict resolution now replays only append-only thread additions and fails safe for non-thread or non-append conflicts instead of dropping local changes.
- App and chat async success paths now guard against stale workspace responses before mutating workspace-scoped stores.
- Failed sends only restore the draft/reply/error state if the user is still in the same workspace and input scope.
- Setup can open a registered cached workspace without a session token in read-only/disconnected mode, while keeping reconnect as the sync path.
- Browser-mode e2e fixtures now return workspace-specific cached data, so workspace switches/reloads verify the active workspace rather than matching identical mock content.
- Git tree diff and sync tests cover directory filtering, commit-read error propagation, append-only conflict replay, fail-safe behavior for non-thread conflicts, and remote-deleted thread conflicts before reset.
- Mobile setup rows and workspace-switcher cache controls remain visible and usable on narrow/touch layouts.
