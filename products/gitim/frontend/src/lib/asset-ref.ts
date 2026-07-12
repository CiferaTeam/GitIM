export interface AssetRef {
  version: 1;
  originRuntimeId: string;
  sha256: string;
  name: string;
  mediaType: string;
  size: number;
  width?: number;
  height?: number;
  raw: string;
}

const MAX_REF_BYTES = 1024;
const MAX_NAME_BYTES = 255;
const MAX_MEDIA_TYPE_BYTES = 127;
const MAX_SIZE = 50 * 1024 * 1024;
const MAX_U32 = 0xffff_ffff;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SHA256_RE = /^[0-9a-f]{64}$/;
const MEDIA_TYPE_RE = /^[a-z0-9!#$%&'*+.\-^_`|~]+\/[a-z0-9!#$%&'*+.\-^_`|~]+$/;
const CONTROL_RE = /\p{Cc}/u;
const UTF8 = new TextEncoder();

function utf8Length(value: string): number {
  return UTF8.encode(value).length;
}

function encodeRfc3986(value: string): string {
  return encodeURIComponent(value).replace(/[!'()*]/g, (character) =>
    `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

function isValidName(name: string): boolean {
  const byteLength = utf8Length(name);
  return (
    byteLength >= 1 &&
    byteLength <= MAX_NAME_BYTES &&
    !CONTROL_RE.test(name) &&
    !name.includes("/") &&
    !name.includes("\\")
  );
}

function isValidMediaType(mediaType: string): boolean {
  const byteLength = utf8Length(mediaType);
  return (
    byteLength >= 1 &&
    byteLength <= MAX_MEDIA_TYPE_BYTES &&
    MEDIA_TYPE_RE.test(mediaType)
  );
}

function isCanonicalUnsignedDecimal(value: string, allowZero: boolean): boolean {
  return allowZero ? /^(?:0|[1-9][0-9]*)$/.test(value) : /^[1-9][0-9]*$/.test(value);
}

function parseUnsignedDecimal(
  value: string,
  maximum: number,
  allowZero: boolean,
): number | null {
  if (!isCanonicalUnsignedDecimal(value, allowZero)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed <= maximum ? parsed : null;
}

function assertValidAsset(asset: Omit<AssetRef, "raw">): void {
  const dimensionsPaired = (asset.width === undefined) === (asset.height === undefined);
  const dimensionsValid =
    asset.width === undefined ||
    (Number.isInteger(asset.width) &&
      asset.width > 0 &&
      asset.width <= MAX_U32 &&
      Number.isInteger(asset.height) &&
      asset.height! > 0 &&
      asset.height! <= MAX_U32);

  if (
    asset.version !== 1 ||
    !UUID_RE.test(asset.originRuntimeId) ||
    !SHA256_RE.test(asset.sha256) ||
    !isValidName(asset.name) ||
    !isValidMediaType(asset.mediaType) ||
    !Number.isSafeInteger(asset.size) ||
    asset.size < 0 ||
    asset.size > MAX_SIZE ||
    !dimensionsPaired ||
    !dimensionsValid
  ) {
    throw new RangeError("invalid asset reference fields");
  }
}

export function formatAssetRef(asset: Omit<AssetRef, "raw">): string {
  assertValidAsset(asset);

  let raw: string;
  try {
    raw = `<^v1/${asset.originRuntimeId}/sha256:${asset.sha256}?name=${encodeRfc3986(asset.name)}&type=${encodeRfc3986(asset.mediaType)}&size=${asset.size}`;
  } catch {
    throw new RangeError("invalid asset reference fields");
  }
  if (asset.width !== undefined) {
    raw += `&width=${asset.width}&height=${asset.height}`;
  }
  raw += ">";

  if (utf8Length(raw) > MAX_REF_BYTES) {
    throw new RangeError("asset reference exceeds 1024-byte limit");
  }
  return raw;
}

export function parseAssetRef(raw: string): AssetRef | null {
  if (utf8Length(raw) > MAX_REF_BYTES) return null;

  const match = raw.match(
    /^<\^v1\/([^/]+)\/sha256:([^?]+)\?name=([^&]*)&type=([^&]*)&size=([^&>]+)(?:&width=([^&>]+)&height=([^&>]+))?>$/,
  );
  if (!match) return null;

  const [, originRuntimeId, sha256, encodedName, encodedMediaType, encodedSize, encodedWidth, encodedHeight] = match;
  let name: string;
  let mediaType: string;
  try {
    name = decodeURIComponent(encodedName);
    mediaType = decodeURIComponent(encodedMediaType);
  } catch {
    return null;
  }

  const size = parseUnsignedDecimal(encodedSize, MAX_SIZE, true);
  if (size === null) return null;

  const hasDimensions = encodedWidth !== undefined || encodedHeight !== undefined;
  if ((encodedWidth === undefined) !== (encodedHeight === undefined)) return null;
  const width = hasDimensions ? parseUnsignedDecimal(encodedWidth, MAX_U32, false) : undefined;
  const height = hasDimensions ? parseUnsignedDecimal(encodedHeight, MAX_U32, false) : undefined;
  if (hasDimensions && (width === null || height === null)) return null;

  const asset: Omit<AssetRef, "raw"> = {
    version: 1,
    originRuntimeId,
    sha256,
    name,
    mediaType,
    size,
    ...(width !== undefined && width !== null && { width }),
    ...(height !== undefined && height !== null && { height }),
  };
  try {
    if (formatAssetRef(asset) !== raw) return null;
  } catch {
    return null;
  }
  return { ...asset, raw };
}
