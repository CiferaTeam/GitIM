import { create } from "zustand";

import { formatAssetRef } from "@/lib/asset-ref";
import { normalizedFilename } from "@/lib/asset-utils";
import type { UploadedAsset } from "@/lib/client";

export type AttachmentDraftStatus = "idle" | "uploading" | "sending" | "error";

export interface PendingAttachment {
  readonly id: string;
  readonly file: File;
  readonly previewUrl?: string;
  readonly uploaded?: UploadedAsset;
}

export interface AttachmentDraft {
  readonly generation: number;
  readonly status: AttachmentDraftStatus;
  readonly items: readonly PendingAttachment[];
  readonly error?: string;
}

export type AttachmentRejectionCode =
  | "duplicate"
  | "too_many_items"
  | "file_too_large"
  | "aggregate_too_large"
  | "filename_too_long"
  | "invalid_reference"
  | "invalid_file"
  | "busy";

export interface AttachmentRejection {
  readonly code: AttachmentRejectionCode;
  readonly message: string;
  readonly file: File;
}

export interface AddFilesResult {
  readonly accepted: readonly PendingAttachment[];
  readonly rejected: readonly AttachmentRejection[];
}

export interface UploadedAttachmentMapping {
  readonly id: string;
  readonly asset: UploadedAsset;
}

export interface AttachmentOperationSnapshot {
  readonly key: string;
  readonly generation: number;
  readonly items: readonly Readonly<PendingAttachment>[];
}

interface AttachmentDraftStore {
  readonly drafts: Readonly<Record<string, AttachmentDraft>>;
  addFiles: (key: string, files: readonly File[]) => AddFilesResult;
  removeItem: (key: string, id: string) => boolean;
  beginOperation: (key: string) => AttachmentOperationSnapshot | null;
  markUploaded: (
    key: string,
    generation: number,
    mappings: readonly UploadedAttachmentMapping[],
  ) => boolean;
  markSending: (key: string, generation: number) => boolean;
  failOperation: (key: string, generation: number, error: string) => boolean;
  completeSuccess: (key: string, generation: number) => boolean;
  disposeWorkspace: (workspaceKey: string) => void;
  disposeAll: () => void;
}

const MAX_ITEMS = 10;
const MAX_FILE_BYTES = 50 * 1024 * 1024;
const MAX_AGGREGATE_BYTES = 200 * 1024 * 1024;
const MAX_FILENAME_BYTES = 255;
const MAX_U32 = 0xffff_ffff;
const PLACEHOLDER_ORIGIN = "00000000-0000-0000-0000-000000000000";
const PLACEHOLDER_HASH = "0".repeat(64);
const UTF8 = new TextEncoder();
const RASTER_PREVIEW_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/avif",
]);

const REJECTION_MESSAGES: Record<AttachmentRejectionCode, string> = {
  duplicate: "This file is already attached.",
  too_many_items: "A message can include at most 10 attachments.",
  file_too_large: "Each attachment must be no larger than 50 MiB.",
  aggregate_too_large: "Attachments must be no larger than 200 MiB in total.",
  filename_too_long: "Attachment filenames must be no longer than 255 UTF-8 bytes.",
  invalid_reference: "The attachment filename cannot be represented safely.",
  invalid_file: "The selected file has invalid browser metadata.",
  busy: "Wait for the current attachment operation to finish.",
};
const EMPTY_DRAFTS: Readonly<Record<string, AttachmentDraft>> = Object.freeze({});

export function attachmentDraftKey(workspaceKey: string, scopeKey: string): string {
  return JSON.stringify([workspaceKey, scopeKey]);
}

function selectionId(file: File): string {
  return JSON.stringify([file.name, file.size, file.lastModified, file.type]);
}

function canFormatRuntimeReference(name: string, size: number): boolean {
  const base = {
    version: 1 as const,
    originRuntimeId: PLACEHOLDER_ORIGIN,
    sha256: PLACEHOLDER_HASH,
    name,
    size,
  };
  try {
    formatAssetRef({ ...base, mediaType: "application/octet-stream" });
    formatAssetRef({
      ...base,
      mediaType: "image/jpeg",
      width: MAX_U32,
      height: MAX_U32,
    });
    return true;
  } catch {
    return false;
  }
}

function rejection(file: File, code: AttachmentRejectionCode): AttachmentRejection {
  return Object.freeze({ file, code, message: REJECTION_MESSAGES[code] });
}

function addFilesResult(
  accepted: readonly PendingAttachment[],
  rejected: readonly AttachmentRejection[],
): AddFilesResult {
  return Object.freeze({
    accepted: Object.freeze([...accepted]),
    rejected: Object.freeze([...rejected]),
  });
}

function publishedDraft(
  generation: number,
  status: AttachmentDraftStatus,
  items: readonly PendingAttachment[],
  error?: string,
): AttachmentDraft {
  return Object.freeze({
    generation,
    status,
    items: Object.freeze([...items]),
    ...(error !== undefined && { error }),
  });
}

function replaceDraft(
  drafts: Readonly<Record<string, AttachmentDraft>>,
  key: string,
  draft?: AttachmentDraft,
): Readonly<Record<string, AttachmentDraft>> {
  const next = { ...drafts };
  if (draft === undefined) {
    delete next[key];
  } else {
    next[key] = draft;
  }
  return Object.freeze(next);
}

function createPreviewUrl(file: File): string | undefined {
  if (!RASTER_PREVIEW_TYPES.has(file.type) || typeof URL.createObjectURL !== "function") {
    return undefined;
  }
  try {
    return URL.createObjectURL(file);
  } catch {
    return undefined;
  }
}

function revokeUrls(items: readonly PendingAttachment[]): void {
  const urls = new Set(
    items.flatMap((item) => (item.previewUrl === undefined ? [] : [item.previewUrl])),
  );
  if (typeof URL.revokeObjectURL !== "function") return;
  for (const url of urls) {
    try {
      URL.revokeObjectURL(url);
    } catch {
      // Object URL cleanup is best-effort; state ownership has already ended.
    }
  }
}

function frozenAsset(asset: UploadedAsset): UploadedAsset {
  return Object.freeze({ ...asset });
}

function operationSnapshot(
  key: string,
  generation: number,
  items: readonly PendingAttachment[],
): AttachmentOperationSnapshot {
  const snapshotItems = items.map((item) =>
    Object.freeze({
      ...item,
      ...(item.uploaded !== undefined && { uploaded: frozenAsset(item.uploaded) }),
    }),
  );
  return Object.freeze({
    key,
    generation,
    items: Object.freeze(snapshotItems),
  });
}

function isBusy(draft: AttachmentDraft): boolean {
  return draft.status === "uploading" || draft.status === "sending";
}

function draftWorkspaceKey(key: string): string | undefined {
  try {
    const tuple: unknown = JSON.parse(key);
    return Array.isArray(tuple) && tuple.length === 2 && typeof tuple[0] === "string"
      ? tuple[0]
      : undefined;
  } catch {
    return undefined;
  }
}

export const useAttachmentDraftStore = create<AttachmentDraftStore>((set, get) => {
  let nextGeneration = 1;
  const allocateGeneration = (): number => nextGeneration++;

  return {
    drafts: EMPTY_DRAFTS,

    addFiles: (key, files) => {
      const current = get().drafts[key];
      if (current !== undefined && isBusy(current)) {
        return addFilesResult([], files.map((file) => rejection(file, "busy")));
      }

      const items = current === undefined ? [] : [...current.items];
      const selectedIds = new Set(items.map((item) => item.id));
      const accepted: PendingAttachment[] = [];
      const rejected: AttachmentRejection[] = [];
      let aggregateBytes = items.reduce((sum, item) => sum + item.file.size, 0);

      for (const file of files) {
        const id = selectionId(file);
        if (selectedIds.has(id)) {
          rejected.push(rejection(file, "duplicate"));
          continue;
        }
        selectedIds.add(id);

        if (!Number.isSafeInteger(file.size) || file.size < 0) {
          rejected.push(rejection(file, "invalid_file"));
          continue;
        }
        if (items.length >= MAX_ITEMS) {
          rejected.push(rejection(file, "too_many_items"));
          continue;
        }
        if (file.size > MAX_FILE_BYTES) {
          rejected.push(rejection(file, "file_too_large"));
          continue;
        }
        if (!Number.isSafeInteger(aggregateBytes + file.size)) {
          rejected.push(rejection(file, "invalid_file"));
          continue;
        }
        if (aggregateBytes + file.size > MAX_AGGREGATE_BYTES) {
          rejected.push(rejection(file, "aggregate_too_large"));
          continue;
        }

        const name = normalizedFilename(file.name);
        if (UTF8.encode(name).length > MAX_FILENAME_BYTES) {
          rejected.push(rejection(file, "filename_too_long"));
          continue;
        }
        if (!canFormatRuntimeReference(name, file.size)) {
          rejected.push(rejection(file, "invalid_reference"));
          continue;
        }

        const previewUrl = createPreviewUrl(file);
        const item = Object.freeze({
          id,
          file,
          ...(previewUrl !== undefined && { previewUrl }),
        });
        items.push(item);
        accepted.push(item);
        aggregateBytes += file.size;
      }

      if (accepted.length > 0 || rejected.length > 0) {
        const generation = current?.generation ?? allocateGeneration();
        const validationError = rejected[0]?.message;
        set((state) => ({
          drafts: replaceDraft(
            state.drafts,
            key,
            publishedDraft(
              generation,
              validationError === undefined ? "idle" : "error",
              items,
              validationError,
            ),
          ),
        }));
      }

      return addFilesResult(accepted, rejected);
    },

    removeItem: (key, id) => {
      const current = get().drafts[key];
      if (current === undefined || isBusy(current)) return false;
      const index = current.items.findIndex((item) => item.id === id);
      if (index < 0) return false;

      const removed = current.items[index];
      const items = current.items.filter((_, itemIndex) => itemIndex !== index);
      const draft =
        items.length === 0
          ? undefined
          : publishedDraft(current.generation, "idle", items);
      set((state) => ({ drafts: replaceDraft(state.drafts, key, draft) }));
      revokeUrls([removed]);
      return true;
    },

    beginOperation: (key) => {
      const current = get().drafts[key];
      if (current === undefined || current.items.length === 0 || isBusy(current)) return null;

      const generation = allocateGeneration();
      set((state) => ({
        drafts: replaceDraft(
          state.drafts,
          key,
          publishedDraft(generation, "uploading", current.items),
        ),
      }));
      return operationSnapshot(key, generation, current.items);
    },

    markUploaded: (key, generation, mappings) => {
      const current = get().drafts[key];
      if (
        current === undefined ||
        current.generation !== generation ||
        current.status !== "uploading"
      ) {
        return false;
      }

      const pendingItems = new Map(
        current.items
          .filter((item) => item.uploaded === undefined)
          .map((item) => [item.id, item] as const),
      );
      const mappedIds = new Set<string>();
      for (const mapping of mappings) {
        const item = pendingItems.get(mapping.id);
        if (
          item === undefined ||
          mappedIds.has(mapping.id) ||
          mapping.asset.name !== normalizedFilename(item.file.name) ||
          mapping.asset.size !== item.file.size
        ) {
          return false;
        }
        mappedIds.add(mapping.id);
      }
      if (mappedIds.size !== pendingItems.size) return false;

      const assets = new Map(mappings.map((mapping) => [mapping.id, frozenAsset(mapping.asset)]));
      const items = current.items.map((item) => {
        const asset = assets.get(item.id);
        return asset === undefined ? item : Object.freeze({ ...item, uploaded: asset });
      });
      set((state) => ({
        drafts: replaceDraft(
          state.drafts,
          key,
          publishedDraft(current.generation, current.status, items, current.error),
        ),
      }));
      return true;
    },

    markSending: (key, generation) => {
      const current = get().drafts[key];
      if (
        current === undefined ||
        current.generation !== generation ||
        current.status !== "uploading" ||
        current.items.some((item) => item.uploaded === undefined)
      ) {
        return false;
      }
      set((state) => ({
        drafts: replaceDraft(
          state.drafts,
          key,
          publishedDraft(current.generation, "sending", current.items),
        ),
      }));
      return true;
    },

    failOperation: (key, generation, error) => {
      const current = get().drafts[key];
      if (
        current === undefined ||
        current.generation !== generation ||
        !isBusy(current)
      ) {
        return false;
      }
      set((state) => ({
        drafts: replaceDraft(
          state.drafts,
          key,
          publishedDraft(current.generation, "error", current.items, error),
        ),
      }));
      return true;
    },

    completeSuccess: (key, generation) => {
      const current = get().drafts[key];
      if (
        current === undefined ||
        current.generation !== generation ||
        current.status !== "sending"
      ) {
        return false;
      }
      set((state) => ({ drafts: replaceDraft(state.drafts, key) }));
      revokeUrls(current.items);
      return true;
    },

    disposeWorkspace: (workspaceKey) => {
      const drafts = get().drafts;
      const disposed = Object.entries(drafts).filter(
        ([key]) => draftWorkspaceKey(key) === workspaceKey,
      );
      if (disposed.length === 0) return;
      const next = Object.fromEntries(
        Object.entries(drafts).filter(([key]) => draftWorkspaceKey(key) !== workspaceKey),
      );
      set({ drafts: Object.freeze(next) });
      revokeUrls(disposed.flatMap(([, draft]) => draft.items));
    },

    disposeAll: () => {
      const drafts = get().drafts;
      set({ drafts: EMPTY_DRAFTS });
      revokeUrls(Object.values(drafts).flatMap((draft) => draft.items));
    },
  };
});
