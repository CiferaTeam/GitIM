# Image Lightbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace new-tab image attachment navigation with an accessible in-app lightbox.

**Architecture:** Keep attachment resolution and failure handling in `AssetFragment`. Convert the successful image card into a Radix Dialog trigger and render a viewport-bounded second image inside the existing dialog primitives, which provide portal rendering, focus trapping, Escape handling, outside-click close, and focus restoration.

**Tech Stack:** React 19, Radix UI Dialog, Tailwind CSS, Vitest, jsdom.

---

### Task 1: Lock the lightbox contract with interaction tests

**Files:**
- Modify: `products/gitim/frontend/src/components/chat/message-body.test.tsx`

- [x] **Step 1: Replace the new-tab assertion with a failing lightbox test**

Replace the link assertions in the canonical PNG test with:

```tsx
const trigger = container.querySelector<HTMLButtonElement>(
  'button[aria-label="Open image preview for fleet-assets.png"]',
);
expect(trigger).not.toBeNull();
expect(image?.closest("a")).toBeNull();
```

Add this interaction test:

```tsx
it("opens an in-app lightbox with image metadata and download", async () => {
  await renderBody(assetRef({ width: 1600, height: 900 }));
  const trigger = container.querySelector<HTMLButtonElement>(
    'button[aria-label="Open image preview for fleet-assets.png"]',
  )!;

  await act(async () => {
    trigger.click();
    await Promise.resolve();
  });

  const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
  expect(dialog).not.toBeNull();
  expect(dialog?.querySelector('[data-slot="dialog-title"]')?.textContent)
    .toBe("fleet-assets.png");
  expect(dialog?.textContent).toContain("180 KiB");
  expect(dialog?.querySelector<HTMLImageElement>("img[data-asset-lightbox-image]")?.src)
    .toContain("/assets/resolve/");
  expect(dialog?.querySelector<HTMLAnchorElement>('a[aria-label="Download fleet-assets.png"]')?.href)
    .toContain("download=1");
  expect(dialog?.querySelector('button[aria-label="Close image preview"]')).not.toBeNull();
});
```

- [x] **Step 2: Add a failing keyboard-close test**

Add:

```tsx
it("closes the image lightbox with Escape and restores trigger focus", async () => {
  await renderBody(assetRef({ width: 1600, height: 900 }));
  const trigger = container.querySelector<HTMLButtonElement>(
    'button[aria-label="Open image preview for fleet-assets.png"]',
  )!;
  trigger.focus();
  await act(async () => {
    trigger.click();
    await Promise.resolve();
  });

  act(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  expect(document.body.querySelector('[role="dialog"]')).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
```

- [x] **Step 3: Update the click-boundary regression test**

Change the interactive lookup to:

```tsx
const imageTrigger = container.querySelector<HTMLButtonElement>(
  'button[aria-label^="Open image preview for "]',
)!;
```

Use `imageTrigger` in the existing target array. Keep the current assertions
that the events are not default-prevented and do not reach the message wrapper.

- [x] **Step 4: Verify RED**

Run:

```bash
npm exec vitest -- run src/components/chat/message-body.test.tsx
```

Expected: FAIL because the current image is an `_blank` anchor and no dialog is
rendered.

### Task 2: Implement the accessible lightbox

**Files:**
- Modify: `products/gitim/frontend/src/components/chat/asset-fragment.tsx`
- Modify: `products/gitim/frontend/src/components/ui/dialog.tsx`
- Test: `products/gitim/frontend/src/components/chat/message-body.test.tsx`

- [x] **Step 1: Build the trigger and dialog**

Import `X` and the existing dialog primitives:

```tsx
import { Download, FileText, RefreshCw, X } from "lucide-react";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "../ui/dialog";
```

Replace the successful image anchor with:

```tsx
<Dialog>
  <DialogTrigger asChild>
    <button
      type="button"
      aria-label={`Open image preview for ${asset.name}`}
      onClick={stopAssetEvent}
      onDoubleClick={stopAssetEvent}
      className="block w-full overflow-hidden rounded-lg border border-border-strong bg-surface text-left text-inherit"
    >
      <span
        data-asset-frame
        className={`relative flex max-h-[440px] max-w-[440px] min-h-20 items-center justify-center overflow-hidden bg-background ${geometry ? "mx-auto" : "w-full"}`}
        style={geometry ? {
          aspectRatio: geometry.aspectRatio,
          width: "100%",
          maxWidth: `${geometry.frameWidth}px`,
        } : undefined}
      >
        <img
          key={attempt}
          data-asset-image
          src={url}
          crossOrigin="anonymous"
          loading="lazy"
          alt={asset.name}
          {...(geometry && { width: geometry.width, height: geometry.height })}
          onLoad={() => setState("loaded")}
          onError={() => setState("unavailable")}
          className="block h-full max-h-[440px] w-full max-w-full object-contain"
        />
        {state === "loading" && (
          <span
            role="status"
            className="absolute inset-0 flex items-center justify-center bg-background/70 text-xs text-text-muted"
          >
            Loading {asset.name}…
          </span>
        )}
      </span>
      <span className="flex items-center justify-between gap-3 border-t border-border px-3 py-2 text-xs text-text-muted">
        <span className="min-w-0 truncate font-medium text-foreground" title={asset.name}>
          {asset.name}
        </span>
        <span className="shrink-0">{formatBinarySize(asset.size)}</span>
      </span>
    </button>
  </DialogTrigger>
  <DialogContent
    showCloseButton={false}
    overlayProps={assetPortalEventBoundary}
    {...assetPortalEventBoundary}
    className="flex h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] max-w-none flex-col gap-0 overflow-hidden border-border-strong bg-background/95 p-0 sm:max-w-none"
  >
    <div className="flex h-12 shrink-0 items-center gap-3 border-b border-border px-3">
      <DialogTitle className="min-w-0 flex-1 truncate text-sm" title={asset.name}>
        {asset.name}
      </DialogTitle>
      <DialogDescription className="sr-only">
        Full-size image preview
      </DialogDescription>
      <span className="shrink-0 text-xs text-text-muted">
        {formatBinarySize(asset.size)}
      </span>
      <DownloadLink asset={asset} url={downloadUrl} />
      <DialogClose asChild>
        <button type="button" aria-label="Close image preview" className="rounded-md p-2 text-text-secondary hover:bg-surface-hover hover:text-foreground">
          <X aria-hidden="true" className="size-4" />
        </button>
      </DialogClose>
    </div>
    <div className="flex min-h-0 flex-1 items-center justify-center p-3 sm:p-6">
      <img
        data-asset-lightbox-image
        src={url}
        crossOrigin="anonymous"
        alt={asset.name}
        onError={() => setState("unavailable")}
        className="max-h-full max-w-full object-contain"
      />
    </div>
  </DialogContent>
</Dialog>
```

- [x] **Step 2: Preserve lifecycle boundaries**

Keep `state`, `attempt`, `imageGeometry`, the loading overlay, and the
`UnavailableCard` branch unchanged. The trigger calls `stopAssetEvent`, while
the outer `data-asset-root` boundary continues to stop propagation without
calling `preventDefault`. Extend the shared `DialogContent` with typed
`overlayProps`, then apply the same click, double-click, touch, and context-menu
propagation boundary to both the portaled content and overlay. This prevents
React portal events from reaching the owning mobile `MessageItem` and opening
its reply, thread, or long-press action sheet interactions.

- [x] **Step 3: Verify GREEN**

Run:

```bash
npm exec vitest -- run src/components/chat/message-body.test.tsx
```

Expected: 23 tests pass with no warnings.

### Task 3: Align the product contract and release checks

**Files:**
- Modify: `docs/plans/file-attachments/00-requirements.md`
- Modify: `docs/plans/image-lightbox/01-implementation-plan.md`
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`

- [x] **Step 1: Update the attachment rendering requirement**

Replace the rendering bullet with:

```markdown
- Clicking an image opens an in-app lightbox with a viewport-bounded preview,
  close controls, and a Download action.
```

- [x] **Step 2: Run frontend verification**

Run:

```bash
npm exec vitest -- run src/components/chat/message-body.test.tsx
npm run lint
npm run build
```

Expected: all commands exit 0.

- [x] **Step 3: Add and run browser E2E**

Extend the mobile attachment Playwright flow to open the rendered image and
assert the dialog/image stay within the 390 × 844 viewport, no popup opens,
Download contains `download=1`, focus remains trapped, close/backdrop/Escape all
dismiss, focus returns to the thumbnail, and long presses on both the portaled
image and overlay do not open the underlying message action sheet. Repeat the
same interaction against a real local Runtime with Kimi WebBridge.

## Verification

- `message-body.test.tsx`: 23 tests passed.
- Frontend suite: 74 files and 716 tests passed.
- Mobile Playwright suite: 19 tests passed, including the lightbox interaction
  and viewport regression.
- ESLint and the production TypeScript/Vite build passed.
- Kimi WebBridge exercised the worktree Vite build against the local Runtime on
  desktop and a 390 × 844 mobile viewport. The dialog stayed within the viewport,
  preserved page width, opened without creating a browser tab, closed through
  Escape, the close action, and the backdrop, and restored focus to the thumbnail.
- The Download action resolved to the local Runtime with `download=1`; HEAD
  returned 200, `image/png`, the expected 88,510-byte length, and attachment
  content disposition.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | Not run | User supplied the interaction contract |
| Codex Review | `/codex review` | Independent 2nd opinion | 2 | CLEAR | 1 portal-event finding, 1/1 fixed |
| Eng Review | SOP independent review | Architecture & tests | 4 | CLEAR | 3 findings, 3/3 fixed |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | Not run | Existing design system and browser QA applied |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | Not applicable | No developer-facing workflow change |

- **CODEX:** Prevented portaled lightbox content and overlay events from opening
  the underlying mobile message action sheet; the clean rerun found no actionable
  correctness issues.
- **CROSS-MODEL:** The engineering review found visible-metadata and diagnostic
  focus-test gaps; Codex uniquely found portal event leakage. All findings are
  fixed and both reviews are clear.
- **UNRESOLVED:** 0.
- **VERDICT:** ENG + CODEX CLEARED — ready to merge.
