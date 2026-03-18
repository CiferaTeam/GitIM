/**
 * GitIM Client — 单进程启动前端 + daemon 代理
 *
 * 开发模式: Vite HMR + WebSocket 代理
 * 生产模式: 静态文件 + WebSocket 代理
 *
 * 用法: npx tsx app.ts --repo ~/ateam/chatroom [--port 3000]
 */

import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createConnection, type Socket } from "node:net";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";
import { readFile, stat } from "node:fs/promises";
import { join, extname, resolve } from "node:path";
import { execSync } from "node:child_process";
import { WebSocketServer, WebSocket } from "ws";

// ─── 类型定义 ───

interface ClientRequest {
  id: number;
  method: string;
  [key: string]: unknown;
}

interface ClientResponse {
  id: number;
  ok: boolean;
  data?: unknown;
  error?: string;
}

interface PushEvent {
  event: string;
  [key: string]: unknown;
}

interface DaemonResponse {
  ok: boolean;
  data?: unknown;
  error?: string;
}

interface PendingRequest {
  internalId: number;
  clientId: number;
  ws: WebSocket;
  timer: ReturnType<typeof setTimeout>;
}

// ─── 参数解析 ───

function parseArgs(): { repo: string; port: number } {
  const args = process.argv.slice(2);
  let repo = process.cwd();
  let port = 3000;

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--repo" && args[i + 1]) {
      repo = resolve(args[++i]);
    } else if (args[i] === "--port" && args[i + 1]) {
      port = parseInt(args[++i], 10);
    }
  }

  return { repo, port };
}

const { repo, port } = parseArgs();
const SOCK_PATH = join(repo, ".gitim", "run", "gitim.sock");
const ME_JSON_PATH = join(repo, ".gitim", "me.json");
const STATIC_DIR = join(import.meta.dirname!, "dist", "public");
const IS_PRODUCTION = process.env.NODE_ENV === "production";

const REQUEST_TIMEOUT_MS = 10_000;
const RECONNECT_DELAY_MS = 3_000;
const MAX_MESSAGE_SIZE = 1 * 1024 * 1024;

const DAEMON_METHODS = new Set([
  "channels", "users", "read", "send", "thread", "status",
]);

// ─── Daemon 连接管理 ───

let daemonSocket: Socket | null = null;
let daemonRL: ReadlineInterface | null = null;
let subscribed = false;

const pendingQueue: PendingRequest[] = [];
let nextInternalId = 1;

const wsClients = new Set<WebSocket>();

function connectDaemon(): void {
  if (daemonSocket) return;

  console.log(`[daemon] 正在连接 ${SOCK_PATH}...`);
  const sock = createConnection(SOCK_PATH);
  daemonSocket = sock;

  sock.on("connect", () => {
    console.log("[daemon] 已连接");
    subscribed = false;
    daemonRL = createInterface({ input: sock });
    daemonRL.on("line", handleDaemonLine);
    sendToDaemon({ method: "subscribe" });
  });

  sock.on("error", (err) => {
    console.error("[daemon] 连接错误:", err.message);
  });

  sock.on("close", () => {
    console.log("[daemon] 连接断开，3s 后重连...");
    cleanup();
    setTimeout(connectDaemon, RECONNECT_DELAY_MS);
  });
}

function cleanup(): void {
  daemonRL?.close();
  daemonRL = null;
  daemonSocket?.destroy();
  daemonSocket = null;
  subscribed = false;

  for (const pending of pendingQueue) {
    clearTimeout(pending.timer);
    safeSend(pending.ws, { id: pending.clientId, ok: false, error: "daemon 连接断开" });
  }
  pendingQueue.length = 0;
}

function sendToDaemon(msg: Record<string, unknown>): boolean {
  if (!daemonSocket || daemonSocket.destroyed) return false;
  daemonSocket.write(JSON.stringify(msg) + "\n");
  return true;
}

function handleDaemonLine(line: string): void {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(line);
  } catch {
    console.warn("[daemon] 无法解析行:", line);
    return;
  }

  if (!subscribed && "ok" in parsed) {
    subscribed = true;
    console.log("[daemon] subscribe 成功");
    return;
  }

  if ("event" in parsed) {
    broadcastEvent(parsed as unknown as PushEvent);
    return;
  }

  const pending = pendingQueue.shift();
  if (pending) {
    clearTimeout(pending.timer);
    const resp = parsed as unknown as DaemonResponse;
    safeSend(pending.ws, { id: pending.clientId, ok: resp.ok, data: resp.data, error: resp.error });
  } else {
    console.warn("[daemon] 收到无匹配的响应:", line);
  }
}

function broadcastEvent(event: PushEvent): void {
  const msg = JSON.stringify(event);
  for (const ws of wsClients) {
    if (ws.readyState === WebSocket.OPEN) {
      try { ws.send(msg); } catch {}
    }
  }
}

// ─── 本地 API ───

async function handleMe(): Promise<ClientResponse["data"]> {
  try {
    const content = await readFile(ME_JSON_PATH, "utf-8");
    const info = JSON.parse(content);
    return { handler: info.handler, display_name: info.display_name };
  } catch {
    try {
      const name = execSync("git config user.name", { cwd: repo, encoding: "utf-8" }).trim();
      const handler = name.toLowerCase().replace(/[^a-z0-9-]/g, "-");
      return { handler, display_name: name };
    } catch {
      return { handler: "unknown", display_name: "Unknown" };
    }
  }
}

// ─── WebSocket 请求处理 ───

async function handleClientMessage(ws: WebSocket, raw: string): Promise<void> {
  let req: ClientRequest;
  try {
    req = JSON.parse(raw);
  } catch {
    safeSend(ws, { id: 0, ok: false, error: "JSON 解析失败" });
    return;
  }

  const { id, method } = req;

  if (typeof id !== "number" || typeof method !== "string") {
    safeSend(ws, { id: id ?? 0, ok: false, error: "缺少 id 或 method" });
    return;
  }

  if (method === "me") {
    try {
      const data = await handleMe();
      safeSend(ws, { id, ok: true, data });
    } catch (err) {
      safeSend(ws, { id, ok: false, error: err instanceof Error ? err.message : "未知错误" });
    }
    return;
  }

  if (!DAEMON_METHODS.has(method)) {
    safeSend(ws, { id, ok: false, error: `未知方法: ${method}` });
    return;
  }

  if (!daemonSocket || daemonSocket.destroyed || !subscribed) {
    safeSend(ws, { id, ok: false, error: "daemon 未连接" });
    return;
  }

  const { id: _id, ...params } = req;
  if (!sendToDaemon(params)) {
    safeSend(ws, { id, ok: false, error: "daemon 发送失败" });
    return;
  }

  const internalId = nextInternalId++;
  const timer = setTimeout(() => {
    const idx = pendingQueue.findIndex((p) => p.internalId === internalId);
    if (idx !== -1) {
      pendingQueue.splice(idx, 1);
      safeSend(ws, { id, ok: false, error: "请求超时" });
    }
  }, REQUEST_TIMEOUT_MS);

  pendingQueue.push({ internalId, clientId: id, ws, timer });
}

function safeSend(ws: WebSocket, data: ClientResponse | PushEvent): void {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(data));
  }
}

// ─── 静态文件服务（生产模式） ───

const MIME_TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
};

async function serveStatic(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = req.url ?? "/";
  let filePath = join(STATIC_DIR, url === "/" ? "index.html" : url);

  if (!filePath.startsWith(STATIC_DIR)) {
    res.writeHead(403);
    res.end("Forbidden");
    return;
  }

  try {
    const fileStat = await stat(filePath);
    if (fileStat.isDirectory()) filePath = join(filePath, "index.html");
  } catch {
    filePath = join(STATIC_DIR, "index.html");
  }

  try {
    const content = await readFile(filePath);
    const ext = extname(filePath);
    res.writeHead(200, { "Content-Type": MIME_TYPES[ext] ?? "application/octet-stream" });
    res.end(content);
  } catch {
    res.writeHead(404);
    res.end("Not Found");
  }
}

// ─── 启动 ───

async function start(): Promise<void> {
  const httpServer = createHttpServer();

  // WebSocket 用 noServer 模式，手动路由 /ws
  const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_MESSAGE_SIZE });

  wss.on("connection", (ws) => {
    console.log("[ws] 新客户端连接");
    wsClients.add(ws);

    ws.on("message", (raw) => {
      const msg = typeof raw === "string" ? raw : raw.toString("utf-8");
      handleClientMessage(ws, msg).catch((err) => {
        console.error("[ws] 处理消息异常:", err);
      });
    });

    ws.on("close", () => {
      wsClients.delete(ws);
      for (let i = pendingQueue.length - 1; i >= 0; i--) {
        if (pendingQueue[i].ws === ws) {
          clearTimeout(pendingQueue[i].timer);
          pendingQueue.splice(i, 1);
        }
      }
    });

    ws.on("error", (err) => {
      console.error("[ws] 客户端错误:", err.message);
    });
  });

  if (IS_PRODUCTION) {
    // 生产模式：静态文件
    httpServer.on("request", (req, res) => {
      serveStatic(req, res).catch(() => {
        res.writeHead(500);
        res.end("Internal Server Error");
      });
    });
  } else {
    // 开发模式：嵌入 Vite dev server
    const { createServer: createViteServer } = await import("vite");
    const vite = await createViteServer({
      server: {
        middlewareMode: true,
        hmr: { port: 24678 },  // HMR 独立端口，不与 /ws 冲突
      },
      appType: "spa",
    });
    httpServer.on("request", (req, res) => {
      vite.middlewares(req, res);
    });
  }

  // 手动路由 WebSocket upgrade 事件
  httpServer.on("upgrade", (req, socket, head) => {
    if (req.url === "/ws") {
      wss.handleUpgrade(req, socket, head, (ws) => {
        wss.emit("connection", ws, req);
      });
    }
    // 其他路径（如 Vite HMR）由 Vite 自行处理
  });

  httpServer.listen(port, () => {
    const url = `http://localhost:${port}`;
    console.log(`[gitim-client] ${IS_PRODUCTION ? "生产" : "开发"}模式`);
    console.log(`[gitim-client] ${url}`);
    console.log(`[gitim-client] 仓库: ${repo}`);
    connectDaemon();
  });
}

start().catch((err) => {
  console.error("启动失败:", err);
  process.exit(1);
});
