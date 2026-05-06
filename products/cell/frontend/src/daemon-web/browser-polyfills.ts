import { Buffer } from "buffer";

type WorkerGlobal = typeof globalThis & {
  Buffer?: typeof Buffer;
  global?: typeof globalThis;
};

const workerGlobal = globalThis as WorkerGlobal;

workerGlobal.Buffer ??= Buffer;
workerGlobal.global ??= globalThis;
