import { useMemo, useState, type SyntheticEvent } from "react";
import { Download, FileText, RefreshCw, X } from "lucide-react";

import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import type { AssetRef } from "../../lib/asset-ref";
import { assetResolveUrl } from "../../lib/client";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "../ui/dialog";

const INLINE_IMAGE_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/avif",
]);

const MAX_DIMENSION_HINT = 4096;
const MAX_FRAME_SIZE = 440;
const MAX_INLINE_IMAGE_AXIS = 32_768;
const MAX_INLINE_IMAGE_PIXELS = 100_000_000;
const MAX_ASPECT_RATIO = 4;
const MIN_ASPECT_RATIO = 1 / MAX_ASPECT_RATIO;

interface ImageGeometry {
  aspectRatio: string;
  frameWidth: number;
  width: number;
  height: number;
}

function imageGeometry(width?: number, height?: number): ImageGeometry | undefined {
  if (width === undefined || height === undefined) return undefined;

  const ratio = width / height;
  if (ratio > MAX_ASPECT_RATIO) {
    return {
      aspectRatio: "4 / 1",
      frameWidth: MAX_FRAME_SIZE,
      width: 1024,
      height: 256,
    };
  }
  if (ratio < MIN_ASPECT_RATIO) {
    return {
      aspectRatio: "1 / 4",
      frameWidth: MAX_FRAME_SIZE * MIN_ASPECT_RATIO,
      width: 256,
      height: 1024,
    };
  }

  const scale = Math.min(1, MAX_DIMENSION_HINT / width, MAX_DIMENSION_HINT / height);
  return {
    aspectRatio: `${width} / ${height}`,
    frameWidth: ratio >= 1 ? MAX_FRAME_SIZE : MAX_FRAME_SIZE * ratio,
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}

function formatBinarySize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MiB`;
}

function shortOrigin(originRuntimeId: string): string {
  return `${originRuntimeId.slice(0, 8)}…`;
}

function stopAssetEvent(event: SyntheticEvent<HTMLElement>) {
  event.stopPropagation();
}

const assetPortalEventBoundary = {
  onClick: stopAssetEvent,
  onDoubleClick: stopAssetEvent,
  onTouchStart: stopAssetEvent,
  onTouchEnd: stopAssetEvent,
  onTouchCancel: stopAssetEvent,
  onTouchMove: stopAssetEvent,
  onContextMenu: stopAssetEvent,
};

function isInlineSafeImage(asset: AssetRef): boolean {
  const { width, height } = asset;
  return INLINE_IMAGE_TYPES.has(asset.mediaType)
    && width !== undefined
    && height !== undefined
    && width <= MAX_INLINE_IMAGE_AXIS
    && height <= MAX_INLINE_IMAGE_AXIS
    && width * height <= MAX_INLINE_IMAGE_PIXELS;
}

function AssetMetadata({ asset }: { asset: AssetRef }) {
  return (
    <span className="min-w-0 flex-1">
      <span
        className="block truncate text-xs font-medium leading-tight text-foreground"
        title={asset.name}
      >
        {asset.name}
      </span>
      <span className="mt-1 block truncate text-xs leading-tight text-text-muted">
        {asset.mediaType} · {formatBinarySize(asset.size)} ·{" "}
        <span
          title={asset.originRuntimeId}
          aria-label={`Origin runtime ${asset.originRuntimeId}`}
        >
          Origin {shortOrigin(asset.originRuntimeId)}
        </span>
      </span>
    </span>
  );
}

function MetadataCard({ asset }: { asset: AssetRef }) {
  return (
    <span
      role="status"
      aria-label={`${asset.name}: Runtime required`}
      className="flex items-center gap-2.5 rounded-lg border border-dashed border-border-strong bg-surface p-3"
    >
      <FileText aria-hidden="true" className="size-5 shrink-0 text-text-muted" />
      <AssetMetadata asset={asset} />
      <span className="flex shrink-0 flex-col items-end gap-1.5">
        <span className="text-xs text-text-muted">Runtime required</span>
        <button
          type="button"
          disabled
          aria-label={`Download ${asset.name}`}
          className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-text-faint disabled:cursor-not-allowed"
        >
          <Download aria-hidden="true" className="size-3" />
          Download
        </button>
      </span>
    </span>
  );
}

function FileCard({ asset, url }: { asset: AssetRef; url: string }) {
  return (
    <span className="flex items-center gap-2.5 rounded-lg border border-border-strong bg-surface p-3">
      <FileText aria-hidden="true" className="size-5 shrink-0 text-text-muted" />
      <AssetMetadata asset={asset} />
      <DownloadLink asset={asset} url={url} />
    </span>
  );
}

function DownloadLink({ asset, url }: { asset: AssetRef; url: string }) {
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={`Download ${asset.name}`}
      title={`Download ${asset.name}`}
      onClick={stopAssetEvent}
      onDoubleClick={stopAssetEvent}
      className="inline-flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-1 text-xs font-medium text-text-secondary transition-colors duration-75 hover:bg-surface-hover hover:text-foreground"
    >
      <Download aria-hidden="true" className="size-3" />
      Download
    </a>
  );
}

function UnavailableCard({
  asset,
  downloadUrl,
  onRetry,
}: {
  asset: AssetRef;
  downloadUrl: string;
  onRetry: () => void;
}) {
  return (
    <span
      role="status"
      aria-live="polite"
      className="block rounded-lg border border-dashed border-border-strong bg-surface p-3"
    >
      <span className="block text-xs font-medium text-foreground">Unavailable</span>
      <span className="mt-2 flex items-center gap-2.5">
        <FileText aria-hidden="true" className="size-5 shrink-0 text-text-muted" />
        <AssetMetadata asset={asset} />
        <span className="flex shrink-0 items-center gap-1.5">
          <DownloadLink asset={asset} url={downloadUrl} />
          <button
            type="button"
            aria-label={`Retry loading ${asset.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onRetry();
            }}
            onDoubleClick={stopAssetEvent}
            className="inline-flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-1 text-xs font-medium text-text-secondary transition-colors duration-75 hover:bg-surface-hover hover:text-foreground"
          >
            <RefreshCw aria-hidden="true" className="size-3" />
            Retry
          </button>
        </span>
      </span>
    </span>
  );
}

function ImageCard({
  asset,
  downloadUrl,
  url,
}: {
  asset: AssetRef;
  downloadUrl: string;
  url: string;
}) {
  const [state, setState] = useState<"loading" | "loaded" | "unavailable">("loading");
  const [attempt, setAttempt] = useState(0);
  const geometry = imageGeometry(asset.width, asset.height);

  if (state === "unavailable") {
    return (
      <UnavailableCard
        asset={asset}
        downloadUrl={downloadUrl}
        onRetry={() => {
          setAttempt((current) => current + 1);
          setState("loading");
        }}
      />
    );
  }

  return (
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
            <button
              type="button"
              aria-label="Close image preview"
              className="rounded-md p-2 text-text-secondary transition-colors duration-75 hover:bg-surface-hover hover:text-foreground"
            >
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
  );
}

export function AssetFragment({ asset }: { asset: AssetRef }) {
  const mode = useConnectionStore((state) => state.mode);
  const activeSlug = useWorkspaceStore((state) => state.activeSlug);
  const runtimeCapable = mode === "remote" && activeSlug !== null;
  const isInlineImage = isInlineSafeImage(asset);
  const resolvedUrls = useMemo(() => {
    if (!runtimeCapable || activeSlug === null) return null;
    const downloadUrl = assetResolveUrl(activeSlug, asset, { download: true });
    return {
      downloadUrl,
      viewUrl: isInlineImage ? assetResolveUrl(activeSlug, asset) : downloadUrl,
    };
  }, [activeSlug, asset, isInlineImage, runtimeCapable]);

  return (
    <span
      data-asset-root
      onClick={stopAssetEvent}
      onDoubleClick={stopAssetEvent}
      className="my-2 block w-full max-w-[440px] text-xs"
    >
      {resolvedUrls === null ? (
        <MetadataCard asset={asset} />
      ) : isInlineImage ? (
        <ImageCard
          key={resolvedUrls.viewUrl}
          asset={asset}
          downloadUrl={resolvedUrls.downloadUrl}
          url={resolvedUrls.viewUrl}
        />
      ) : (
        <FileCard asset={asset} url={resolvedUrls.downloadUrl} />
      )}
    </span>
  );
}
