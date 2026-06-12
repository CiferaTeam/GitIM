// Message/event formatting — delegates to the authoritative Rust formatter
// via wasm. Keeps `number`/object signatures for callers; bridges to the
// wasm boundary (u64 -> bigint, event meta -> JSON string).
//
// wasm must be initialized before these run (callers await ensureWasmReady).

import {
  formatMessage as wasmFormatMessage,
  formatEvent as wasmFormatEvent,
} from "gitim-wasm";

export function formatMessage(
  lineNumber: number,
  pointTo: number,
  author: string,
  timestamp: string,
  body: string,
): string {
  return wasmFormatMessage(
    BigInt(lineNumber),
    BigInt(pointTo),
    author,
    timestamp,
    body,
  );
}

export function formatEvent(
  lineNumber: number,
  author: string,
  timestamp: string,
  eventType: string,
  meta: Record<string, unknown>,
): string {
  return wasmFormatEvent(
    BigInt(lineNumber),
    author,
    timestamp,
    eventType,
    JSON.stringify(meta),
  );
}
