import { execSync, spawn, type ChildProcess } from "node:child_process";
import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const ROOT = path.resolve(__dirname, "../..");
export const WEBUI_DIR = path.join(ROOT, "webui-v2");

export interface RuntimeEnv {
  runtimePort: number;
  vitePort: number;
  workspaceDir: string;
  runtimeProc: ChildProcess;
  viteProc: ChildProcess;
  baseUrl: string;
}

/** Find a free port on localhost. */
export async function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address() as net.AddressInfo;
      srv.close(() => resolve(addr.port));
    });
  });
}

/** Wait until the runtime /health endpoint responds with service === "gitim-runtime". */
export async function waitForHealth(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      const data = await res.json();
      if (data.service === "gitim-runtime") return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Runtime did not become healthy at ${url}`);
}

/** Wait until an HTTP endpoint responds with any 200. */
export async function waitForHttp(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Server did not become available at ${url}`);
}

/** Build gitim-runtime and gitim-cli binaries. */
export function buildRuntime(): void {
  execSync("cargo build -p gitim-runtime -p gitim-cli", {
    cwd: ROOT,
    stdio: "inherit",
  });
}

/**
 * Start runtime + vite dev server, complete the startup flow via HTTP
 * (POST /workspace then POST /git/init with provider=local), and return
 * a RuntimeEnv ready for tests to use.
 */
export async function startEnv(): Promise<RuntimeEnv> {
  // Use /tmp directly (not os.tmpdir()) to keep paths short enough for
  // Unix socket name limits (104 chars on macOS). os.tmpdir() on macOS
  // returns /var/folders/... which makes the socket path too long.
  const tmpBase = process.platform === "darwin" ? "/tmp" : os.tmpdir();
  const workspaceDir = fs.mkdtempSync(path.join(tmpBase, "gitim-e2e-"));

  // Start runtime
  const runtimePort = await freePort();
  const runtimeBin = path.join(ROOT, "target/debug/gitim-runtime");
  const debugBinDir = path.join(ROOT, "target/debug");
  // Prepend target/debug to PATH so that MockProvider spawns the correct
  // gitim binary (not an older system-installed version).
  const runtimeEnv = {
    ...process.env,
    PATH: `${debugBinDir}:${process.env.PATH ?? ""}`,
  };
  const runtimeProc = spawn(runtimeBin, ["--port", String(runtimePort)], {
    stdio: "pipe",
    env: runtimeEnv,
  });

  // Start vite dev server
  const vitePort = await freePort();
  const viteProc = spawn(
    "npx",
    ["vite", "--port", String(vitePort), "--strictPort", "--host", "127.0.0.1"],
    {
      cwd: WEBUI_DIR,
      stdio: "pipe",
      env: { ...process.env, BROWSER: "none" },
    },
  );

  // Wait for both servers to be ready
  await Promise.all([
    waitForHealth(`http://127.0.0.1:${runtimePort}/health`),
    waitForHttp(`http://127.0.0.1:${vitePort}`),
  ]);

  // Complete startup flow via HTTP so tests begin in a "ready" state
  const runtimeBase = `http://127.0.0.1:${runtimePort}`;

  const wsRes = await fetch(`${runtimeBase}/workspace`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path: workspaceDir }),
  });
  const wsData = await wsRes.json() as { ok: boolean; error?: string };
  if (!wsData.ok) {
    throw new Error(`POST /workspace failed: ${wsData.error}`);
  }

  const gitRes = await fetch(`${runtimeBase}/git/init`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ provider: "local" }),
  });
  const gitData = await gitRes.json() as { ok: boolean; error?: string };
  if (!gitData.ok) {
    throw new Error(`POST /git/init failed: ${gitData.error}`);
  }

  return {
    runtimePort,
    vitePort,
    workspaceDir,
    runtimeProc,
    viteProc,
    baseUrl: runtimeBase,
  };
}

/**
 * Start runtime + vite WITHOUT completing the startup flow.
 * Use this when the UI test needs to drive the setup itself.
 */
export async function startServers(): Promise<RuntimeEnv> {
  const tmpBase = process.platform === "darwin" ? "/tmp" : os.tmpdir();
  const workspaceDir = fs.mkdtempSync(path.join(tmpBase, "gitim-e2e-"));

  const runtimePort = await freePort();
  const runtimeBin = path.join(ROOT, "target/debug/gitim-runtime");
  const debugBinDir = path.join(ROOT, "target/debug");
  const runtimeProc = spawn(runtimeBin, ["--port", String(runtimePort)], {
    stdio: "pipe",
    env: { ...process.env, PATH: `${debugBinDir}:${process.env.PATH ?? ""}` },
  });

  const vitePort = await freePort();
  const viteProc = spawn(
    "npx",
    ["vite", "--port", String(vitePort), "--strictPort", "--host", "127.0.0.1"],
    {
      cwd: WEBUI_DIR,
      stdio: "pipe",
      env: { ...process.env, BROWSER: "none" },
    },
  );

  await Promise.all([
    waitForHealth(`http://127.0.0.1:${runtimePort}/health`),
    waitForHttp(`http://127.0.0.1:${vitePort}`),
  ]);

  return {
    runtimePort,
    vitePort,
    workspaceDir,
    runtimeProc,
    viteProc,
    baseUrl: `http://127.0.0.1:${runtimePort}`,
  };
}

/** Kill processes and remove the temp workspace directory. */
export function stopEnv(env: RuntimeEnv): void {
  // Kill runtime first — it will gracefully stop all managed daemons on SIGTERM.
  // Wait for it to exit before deleting workspace, otherwise pid files are gone
  // before runtime can read them.
  // Kill runtime first — its graceful shutdown handler kills managed daemons.
  // Brief sleep lets the shutdown complete before we delete the workspace.
  env.runtimeProc?.kill();
  env.viteProc?.kill();
  execSync("sleep 1");

  if (env.workspaceDir && fs.existsSync(env.workspaceDir)) {
    fs.rmSync(env.workspaceDir, { recursive: true, force: true });
  }
}
