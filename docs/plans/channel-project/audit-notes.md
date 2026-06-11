# meta.yaml dispatch-path audit (eng-review finding 1.A)

**Risk:** the new top-level `projects/<slug>.meta.yaml` files (flat, sibling of
`channels/`, `users/`, `dm/`) could be misclassified as channels / users / cards
by any code path that scans top-level dirs or globs `*.meta.yaml` and dispatches
on the result.

**Verdict: no gaps.** Every dispatch/scan site is fenced by either (a) a strict
`strip_prefix` / `starts_with` guard with a default-skip, or (b) a hardcoded
subdirectory scope. No site enumerates the repo root, and no site has a
catch-all `*.meta.yaml` branch. A `projects/<slug>.meta.yaml` change therefore
either matches no prefix (skipped) or never enters the scan set at all.

## Audit table

| Scan / dispatch site | file:line | Predicate | Verdict |
|---|---|---|---|
| poll dispatch ladder | `gitim-daemon/handlers/poll.rs:258-473` | `if/else-if` on `strip_prefix("channels/" \| "dm/" \| "archive/channels/" \| "archive/dm/" \| "crons/" \| "archive/users/")` with terminal `else { continue }` | safe — `projects/` matches no prefix → falls through to `else { continue }`, emits nothing |
| poll membership cache | `gitim-daemon/handlers/poll.rs:128-145` | `extract_channel` returns `Some` only after `strip_prefix("channels/")` | safe — `projects/` → `None` |
| FTS5 indexer | `gitim-index/lib.rs:694-714` (`parse_diff_path`) | `strip_prefix("channels/" \| "dm/")`, else `None`; existing test asserts `users/*.meta.yaml → None` | safe — `projects/*.meta.yaml → None`, not indexed (indexer only acts on `.thread` content anyway) |
| sync conflict-resolver — capture | `gitim-sync/sync_loop.rs:826-835` | `changed_files_unpushed("*.meta.yaml")` glob | captures `projects/` meta (glob is intentional) — see resolve verdict below |
| sync conflict-resolver — resolvable-set gate | `gitim-sync/sync_loop.rs:921-926` | `!ends_with(".meta.yaml")` ⇒ `projects/` is *in* the resolvable set | safe — being resolvable means re-applied (not dropped); correct, see below |
| sync conflict-resolver — meta re-apply | `gitim-sync/sync_loop.rs:996-1056` | `if rel_path.starts_with("channels/")` → `ChannelMeta` member-merge; **else → write local content back as-is** | safe — `projects/` lands in the type-agnostic `else` branch (last-writer-wins, same as `users/`); never parsed as `ChannelMeta`, so no parse-failure drop |
| file watcher | `gitim-sync/watcher.rs:80-82` | strips `.meta.yaml` path-agnostically, BUT watch set is only `channels/` `dm/` `flows/` (lines 49-57) | safe — `projects/` is not in the watch set, so `MetaModified` never fires for it |
| on_synced users refresh | `gitim-daemon/state.rs:481-489` | `read_dir(repo_root.join("users"))` | safe — `users/` dir scoped |
| AppState boot user scan | `gitim-daemon/main.rs:69-79` | `read_dir(repo_root.join("users"))` | safe — `users/` dir scoped |
| onboard user scan (test-path mirror) | `gitim-daemon/onboard.rs:951-958` | `read_dir(repo_root.join("users"))` | safe — `users/` dir scoped; onboard only ever writes `users/` + `channels/general` by construction |
| reconcile orphan cards | `gitim-daemon/reconcile.rs:29-56` | `read_dir(repo_root.join("channels"))` | safe — `channels/` dir scoped |
| archive_channel | `gitim-daemon/handlers/channel.rs:232,256` | operates on constructed `channels/<name>` paths from caller arg; never enumerates top-level | safe — `channels/<name>` scoped |
| unarchive_channel | `gitim-daemon/handlers/channel.rs:563,586` | constructed `archive/channels/<name>` / `channels/<name>` paths | safe — constructed-path scoped |
| list_channels | `gitim-daemon/handlers/read.rs:122,146` | `read_dir(channels/)` + `read_dir(dm/)` | safe — dir scoped |
| list_archived_channels / archived_users | `gitim-daemon/handlers/read.rs:187,247,300` | `read_dir(archive/channels/)` / `archive/users/` | safe — dir scoped |
| labels scan | `gitim-daemon/handlers/labels.rs:362` | `users_dir.join(<handler>.meta.yaml)` over known `active` list | safe — `users/` scoped, iterates known handlers |
| flow list | `gitim-daemon/flow_handlers.rs:23,27` | `read_dir(repo_root.join("flows"))` | safe — `flows/` dir scoped |
| board list | `gitim-daemon/board_handlers.rs:51,56` | `read_dir(repo_root.join("showboards"))` | safe — `showboards/` dir scoped |
| compute_recipients | `gitim-core/recipients.rs:22` | takes `(message, channel_meta, all_messages)` — no path/dir input | safe — cannot see `projects/` |
| frontend channel meta parser | `products/gitim/frontend/src/daemon-web/paths.ts:65` | pure basename stripper; caller (`refreshChannelsCache`) feeds only `channels/` dir contents (verified design §10.1) | safe — caller scopes input to `channels/` |

## Notes

- **Channel `project:` field** (`handle_set_channel_project`) writes a
  `channels/<ch>.meta.yaml` mutation, not a `projects/` file. It flows through
  the existing channel-meta dispatch unchanged; `ChannelMeta`'s optional
  `project` field parses + merges fine (backward-compat covered in core).
- **`archive/projects/`** anchor is v1-unused (zero code references confirmed).
  No archive/unarchive path for projects exists yet, so no archive dispatch
  surface to audit.

No code changes required.
