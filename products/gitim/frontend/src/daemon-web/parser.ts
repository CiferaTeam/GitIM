// Thread parsing — delegates to the authoritative Rust parser via wasm.
// The TS interfaces below describe the serde-serialized shape of
// gitim-core's ThreadFile (the wasm .d.ts only types these as `any`).
//
// wasm must be initialized before calling parseThread. Every daemon-web entry
// point that reaches this code awaits `ensureWasmReady()` first; parseThread
// itself stays synchronous so it can be called inside tight parse loops.

import { parseThread as wasmParseThread } from "gitim-wasm";

export interface ParsedMessage {
  type: "message";
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;
  body: string;
  // gitim-core::Message also serializes `mentions` and `links`; daemon-web
  // doesn't read them, but they're present on the runtime object.
  mentions?: string[];
  links?: unknown[];
}

export interface ParsedEvent {
  type: "event";
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;
  event_type: string;
  meta: Record<string, unknown>;
}

export type ThreadEntry = ParsedMessage | ParsedEvent;

export interface ThreadFile {
  entries: ThreadEntry[];
}

export function parseThread(text: string): ThreadFile {
  return wasmParseThread(text) as ThreadFile;
}
