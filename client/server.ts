/**
 * GitIM Bridge Server
 *
 * 浏览器 ←─WebSocket─→ server.ts ←─Unix Socket─→ daemon
 *                       ↓
 *                静态文件服务 (dist/public/)
 */

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createConnection, type Socket } from "node:net";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";
import { readFile, stat } from "node:fs/promises";
import { join, extname, resolve } from "node:path";
import { execSync } from "node:child_process";
import { WebSocketServer, WebSocket } from "ws";

// ─── 类型定义 ───

/** 浏览器发来的请求 */
interface ClientRequest {
  id: number;
  method: string;
  [key: string]: unknown;
}

/** 发给浏览器的响应 */
interface ClientResponse {
  id: number;
  ok: boolean;
  data?: unknown;
  error?: string;
}

/** 推送事件（无 id） */
interface PushEvent {
  event: string;
  [key: string]: unknown;
}

/** daemon 响应 */
interface DaemonResponse {
  ok: boolean;
  data?: unknown;
  error?: string;
}

/** FIFO 队列中的待处理请求 */
interface PendingRequest {
  clientId: number;
  ws: WebSocket;
  timer: ReturnType<typeof setTimeout>;
}

// ─── 参数解析 ───

function parseArgs(): { repo: string; port: number } {
  const args = process.argv.slice(2);
  let repo = process.cwd();
  let port = 3001;

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
const MAX_MESSAGE_SIZE = 1 * 1024 * 1024; // 1MB

// daemon 透传方法白名单
const DAEMON_METHODS = new Set([
  "channels",
  "users",
  "read",
  "send",
  "thread",
  "status",
]);

// ─── Daemon 连接管理 ───

let daemonSocket: Socket | null = null;
let daemonRL: ReadlineInterface | null = null;
let subscribed = false;

/** FIFO 队列：daemon 不回显 id，按顺序匹配 */
const pendingQueue: PendingRequest[] = [];

/** 所有活跃的 WebSocket 客户端 */
const wsClients = new Set<WebSocket>();

/** 连接 daemon Unix Socket */
function connectDaemon(): void {
  if (daemonSocket) return;

  console.log(`[daemon] 正在连接 ${SOCK_PATH}...`);

  const sock = createConnection(SOCK_PATH);
  daemonSocket = sock;

  sock.on("connect", () => {
    console.log("[daemon] 已连接");
    subscribed = false;

    // 用 readline 逐行读取
    daemonRL = createInterface({ input: sock });
    daemonRL.on("line", handleDaemonLine);

    // 发送 subscribe 进入推送模式
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

/** 清理 daemon 连接状态 */
function cleanup(): void {
  daemonRL?.close();
  daemonRL = null;
  daemonSocket?.destroy();
  daemonSocket = null;
  subscribed = false;

  // 清空待处理队列，通知客户端错误
  for (const pending of pendingQueue) {
    clearTimeout(pending.timer);
    safeSend(pending.ws, {
      id: pending.clientId,
      ok: false,
      error: "daemon 连接断开",
    });
  }
  pendingQueue.length = 0;
}

/** 向 daemon 发送 JSON 行 */
function sendToDaemon(msg: Record<string, unknown>): boolean {
  if (!daemonSocket || daemonSocket.destroyed) return false;
  daemonSocket.write(JSON.stringify(msg) + "\n");
  return true;
}

/** 处理 daemon 返回的每一行 */
function handleDaemonLine(line: string): void {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(line);
  } catch {
    console.warn("[daemon] 无法解析行:", line);
    return;
  }

  // subscribe 的响应
  if (!subscribed && "ok" in parsed) {
    subscribed = true;
    console.log("[daemon] subscribe 成功");
    return;
  }

  // 推送事件（有 event 字段）
  if ("event" in parsed) {
    broadcastEvent(parsed as unknown as PushEvent);
    return;
  }

  // 普通响应 — FIFO 匹配
  const pending = pendingQueue.shift();
  if (pending) {
    clearTimeout(pending.timer);
    const resp = parsed as unknown as DaemonResponse;
    safeSend(pending.ws, {
      id: pending.clientId,
      ok: resp.ok,
      data: resp.data,
      error: resp.error,
    });
  } else {
    console.warn("[daemon] 收到无匹配的响应:", line);
  }
}

/** 向所有 WebSocket 客户端广播推送事件 */
function broadcastEvent(event: PushEvent): void {
  const msg = JSON.stringify(event);
  for (const ws of wsClients) {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(msg);
    }
  }
}

// ─── 本地 API ───

/** 处理 me 请求：读取 me.json 或从 git config 推断 */
async function handleMe(): Promise<ClientResponse["data"]> {
  try {
    const content = await readFile(ME_JSON_PATH, "utf-8");
    const info = JSON.parse(content);
    return { handler: info.handler, display_name: info.display_name };
  } catch {
    // 文件不存在，从 git config 推断
    try {
      const name = execSync("git config user.name", {
        cwd: repo,
        encoding: "utf-8",
      }).trim();
      const handler = name.toLowerCase().replace(/[^a-z0-9-]/g, "-");
      return { handler, display_name: name };
    } catch {
      return { handler: "unknown", display_name: "Unknown" };
    }
  }
}

// ─── WebSocket 请求处理 ───

/** 处理单个客户端请求 */
async function handleClientMessage(
  ws: WebSocket,
  raw: string,
): Promise<void> {
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

  // 本地方法：me
  if (method === "me") {
    try {
      const data = await handleMe();
      safeSend(ws, { id, ok: true, data });
    } catch (err) {
      safeSend(ws, {
        id,
        ok: false,
        error: err instanceof Error ? err.message : "未知错误",
      });
    }
    return;
  }

  // daemon 透传
  if (!DAEMON_METHODS.has(method)) {
    safeSend(ws, { id, ok: false, error: `未知方法: ${method}` });
    return;
  }

  if (!daemonSocket || daemonSocket.destroyed || !subscribed) {
    safeSend(ws, { id, ok: false, error: "daemon 未连接" });
    return;
  }

  // 构造发给 daemon 的请求（去掉 id）
  const { id: _id, ...params } = req;
  const sent = sendToDaemon(params);
  if (!sent) {
    safeSend(ws, { id, ok: false, error: "daemon 发送失败" });
    return;
  }

  // 入队等待响应
  const timer = setTimeout(() => {
    const idx = pendingQueue.findIndex((p) => p.clientId === id && p.ws === ws);
    if (idx !== -1) {
      pendingQueue.splice(idx, 1);
      safeSend(ws, { id, ok: false, error: "请求超时" });
    }
  }, REQUEST_TIMEOUT_MS);

  pendingQueue.push({ clientId: id, ws, timer });
}

/** 安全发送 JSON 到 WebSocket */
function safeSend(ws: WebSocket, data: ClientResponse | PushEvent): void {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(data));
  }
}

// ─── 静态文件服务 ───

const MIME_TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".ttf": "font/ttf",
};

/** 处理静态文件请求 */
async function serveStatic(
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  const url = req.url ?? "/";
  let filePath = join(STATIC_DIR, url === "/" ? "index.html" : url);

  // 防止路径穿越
  if (!filePath.startsWith(STATIC_DIR)) {
    res.writeHead(403);
    res.end("Forbidden");
    return;
  }

  try {
    const fileStat = await stat(filePath);
    // 目录则尝试 index.html
    if (fileStat.isDirectory()) {
      filePath = join(filePath, "index.html");
    }
  } catch {
    // SPA fallback：文件不存在则返回 index.html
    filePath = join(STATIC_DIR, "index.html");
  }

  try {
    const content = await readFile(filePath);
    const ext = extname(filePath);
    const contentType = MIME_TYPES[ext] ?? "application/octet-stream";
    res.writeHead(200, { "Content-Type": contentType });
    res.end(content);
  } catch {
    res.writeHead(404);
    res.end("Not Found");
  }
}

// ─── 启动 ───

const httpServer = createServer(async (req, res) => {
  // 限制请求体大小
  let bodySize = 0;
  req.on("data", (chunk: Buffer) => {
    bodySize += chunk.length;
    if (bodySize > MAX_MESSAGE_SIZE) {
      res.writeHead(413);
      res.end("Request Entity Too Large");
      req.destroy();
    }
  });

  if (IS_PRODUCTION) {
    await serveStatic(req, res);
  } else {
    // 开发模式不 serve 静态文件
    res.writeHead(404);
    res.end("开发模式：请使用 Vite dev server");
  }
});

const wss = new WebSocketServer({
  server: httpServer,
  path: "/ws",
  maxPayload: MAX_MESSAGE_SIZE,
});

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
    console.log("[ws] 客户端断开");
    wsClients.delete(ws);

    // 清理该客户端的待处理请求
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

httpServer.listen(port, () => {
  console.log(`[server] GitIM Bridge Server 已启动`);
  console.log(`[server] 端口: ${port}`);
  console.log(`[server] 模式: ${IS_PRODUCTION ? "生产" : "开发"}`);
  console.log(`[server] 仓库: ${repo}`);
  console.log(`[server] WebSocket: ws://localhost:${port}/ws`);

  // 启动 daemon 连接
  connectDaemon();
});
