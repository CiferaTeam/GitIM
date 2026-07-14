/// <reference types="node" />
// Vitest global setup: initialize the real gitim-wasm module before any test
// runs. wasm-pack's `--target web` output loads the .wasm via fetch by
// default, which fails under node/vitest — so we read the bytes off disk and
// hand them to the init function explicitly.
//
// This makes the daemon-web parser/formatter/conflict/meta tests run against
// the actual Rust logic (the whole point of the wasm convergence) instead of
// a TS re-implementation or a mock.
//
// Path is resolved from process.cwd() (vitest's working dir = the frontend
// package root) rather than import.meta.url: component tests run in jsdom
// where import.meta.url is an http:// URL and fileURLToPath would throw.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll } from "vitest";
import initWasm from "gitim-wasm";

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => Array.from(values.keys())[index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

function ensureTestStorage(name: "localStorage" | "sessionStorage") {
  if (typeof globalThis[name]?.getItem === "function") return;
  Object.defineProperty(globalThis, name, {
    configurable: true,
    value: createMemoryStorage(),
  });
}

// Some Node runtimes expose a partial Web Storage object to jsdom. Normalize
// the test contract before application stores are imported during collection.
ensureTestStorage("localStorage");
ensureTestStorage("sessionStorage");

beforeAll(async () => {
  const wasmPath = resolve(
    process.cwd(),
    "../../../crates/gitim-wasm/pkg/gitim_wasm_bg.wasm",
  );
  await initWasm(readFileSync(wasmPath));
});
