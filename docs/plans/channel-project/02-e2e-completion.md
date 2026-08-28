# Channel Project Grouping E2E Completion Plan

> **For agentic workers:** Implement each task with test-first red/green cycles and review the final diff before integration.

**Goal:** Complete the channel-project workflow in the WebUI so a user can create a project, assign or unassign channels, and keep folded channels exclusively in the bottom Folded section.

**Architecture:** Reuse the existing Git-backed `CreateProject` and `SetChannelProject` runtime APIs. The sidebar owns mutation orchestration and store refreshes, while focused UI components render the project creation dialog and per-channel assignment menu. The existing sidebar tree remains the grouping source after folded channels are removed from its input.

**Tech Stack:** React 19, Zustand, Radix UI, Vitest, Playwright.

## Global Constraints

- Follow `DESIGN.md` tokens and existing sidebar interaction patterns.
- Project assignment remains Git-backed through the existing runtime APIs.
- Folded channels render only in the bottom Folded section and retain their project metadata.
- Browser-local mode keeps project mutations unavailable.
- Validation uses the existing project slug contract: lowercase `a-z`, `0-9`, hyphen, 1-32 characters.

---

### Task 1: Folded project-channel rendering

**Files:**
- Modify: `products/gitim/frontend/src/components/chat/sidebar.tsx`
- Test: `products/gitim/frontend/src/components/chat/sidebar.test.tsx`

**Interfaces:**
- Consumes: `foldedChannels`, `pinnedConversations.channels`, `buildSidebarTree(...)`
- Produces: `visibleRegularChannels: Channel[]`

- [x] Add a failing sidebar test that stores a project child in `gitim-folded-channels:runtime:room`, expands the project, and asserts the channel appears only under `sidebar-folded-channel-list`.
- [x] Run `npm exec vitest -- run src/components/chat/sidebar.test.tsx` and confirm the project child is duplicated before the fix.
- [x] Filter folded, unpinned channels before passing channels to `buildSidebarTree` while retaining the complete list for the Folded section.
- [x] Rerun the scoped test and confirm it passes.

### Task 2: Project creation and channel assignment UI

**Files:**
- Create: `products/gitim/frontend/src/components/chat/channel-project-controls.tsx`
- Modify: `products/gitim/frontend/src/components/chat/sidebar.tsx`
- Modify: `products/gitim/frontend/src/lib/client.ts`
- Test: `products/gitim/frontend/src/components/chat/sidebar.test.tsx`
- Test: `products/gitim/frontend/src/lib/client.project.test.ts`

**Interfaces:**
- Produces: `validateProjectSlug(slug: string): string | null`
- Produces: `CreateProjectDialog` with `onCreate(slug, displayName, introduction)`
- Produces: `ChannelProjectMenu` with `channel`, `projects`, `currentProject`, and `onAssign(project)`
- Consumes: `client.createProject(...)`, `client.setChannelProject(...)`, project store `fetch(...)`, chat store `setChannels(...)`

- [x] Add failing validation tests for reserved project slugs and the shared lowercase/hyphen contract.
- [x] Add failing sidebar tests that submit a new project and assign/unassign a channel through the rendered controls.
- [x] Run the focused tests and confirm failures are caused by missing controls and validator.
- [x] Implement the validator, dialog, assignment menu, mutation handlers, busy/error states, and store refreshes.
- [x] Rerun the focused tests and confirm they pass.

### Task 3: Full browser workflow

**Files:**
- Modify: `products/gitim/frontend/e2e/channel-project.spec.ts`
- Modify: `products/gitim/frontend/package.json`

**Interfaces:**
- Consumes: runtime HTTP contracts `POST /im/projects` and `PATCH /im/channels/{channel}/project`
- Produces: a mutable Playwright runtime stub that reflects project and channel metadata writes in later reads

- [x] Add a failing Playwright scenario for create project, assign channel, fold to the bottom section, unfold back into the project, and unassign to top level.
- [x] Run the scenario against the isolated Vite server and confirm it fails at the first missing control.
- [x] Extend the runtime stub to persist request effects and add `channel-project.spec.ts` to the repository E2E script.
- [x] Rerun the scenario and confirm the entire workflow passes.

### Task 4: Verification and review

**Files:**
- Review all files changed since the worktree base commit.

- [x] Run focused Vitest tests for sidebar, sidebar tree, and client.
- [x] Run `npm run lint`.
- [x] Run `npm run build`.
- [x] Run `channel-project.spec.ts` against an isolated Vite server.
- [x] Run `git diff --check` and inspect the final diff for current-state-only artifacts.
- [x] Request an independent code review and address every critical or important finding.
