// Single wasm init gate for daemon-web.
//
// gitim-wasm's init is async, but the parse/format/validate/conflict business
// functions are synchronous (called inside tight loops). We resolve the init
// promise once, up front, so those synchronous calls are always safe. Every
// entry point that reaches wasm-backed code awaits this first; it's an
// idempotent await on a cached promise, so calling it repeatedly is free.

import initWasm from "gitim-wasm";

let wasmReady: Promise<void> | null = null;

export function ensureWasmReady(): Promise<void> {
  wasmReady ??= initWasm().then(() => undefined);
  return wasmReady;
}
