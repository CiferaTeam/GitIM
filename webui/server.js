// GitIM WebUI Bridge Server
// 连接 daemon Unix Socket，提供 HTTP REST API 和 WebSocket 推送

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// 解析命令行参数
function parseArgs() {
  const args = process.argv.slice(2);
  const opts = { repo: process.cwd(), port: 3000 };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--repo' && args[i + 1]) opts.repo = args[++i];
    if (args[i] === '--port' && args[i + 1]) opts.port = parseInt(args[++i], 10);
  }
  return opts;
}

const opts = parseArgs();
const SOCKET_PATH = path.join(opts.repo, '.gitim', 'run', 'gitim.sock');

// ==================== Daemon 连接 ====================

class DaemonConnection {
  constructor(socketPath) {
    this.socketPath = socketPath;
    this.socket = null;
    this.connected = false;
    this.buffer = '';
    this.pendingRequests = []; // FIFO 队列: [{resolve, reject}]
    this.subscribed = false;
    this.onPush = null; // 推送事件回调
  }

  connect() {
    return new Promise((resolve) => {
      try {
        this.socket = net.createConnection(this.socketPath, () => {
          this.connected = true;
          console.log(`[daemon] 已连接到 ${this.socketPath}`);
          this._setupReadline();
          resolve(true);
        });
        this.socket.on('error', (err) => {
          console.error(`[daemon] 连接错误: ${err.message}`);
          this.connected = false;
          resolve(false);
        });
        this.socket.on('close', () => {
          console.log('[daemon] 连接已关闭');
          this.connected = false;
        });
      } catch (err) {
        console.error(`[daemon] 连接失败: ${err.message}`);
        resolve(false);
      }
    });
  }

  _setupReadline() {
    this.socket.on('data', (chunk) => {
      this.buffer += chunk.toString();
      let idx;
      while ((idx = this.buffer.indexOf('\n')) !== -1) {
        const line = this.buffer.slice(0, idx).trim();
        this.buffer = this.buffer.slice(idx + 1);
        if (!line) continue;
        try {
          const json = JSON.parse(line);
          // 区分推送事件和请求响应
          if (json.event && this.subscribed) {
            // 推送事件
            if (this.onPush) this.onPush(json);
          } else if (this.pendingRequests.length > 0) {
            // 请求响应（FIFO）
            const { resolve } = this.pendingRequests.shift();
            resolve(json);
          }
        } catch (e) {
          console.error('[daemon] JSON 解析失败:', line);
        }
      }
    });
  }

  // 发送请求并等待响应
  async request(msg) {
    if (!this.connected) {
      return { ok: false, error: 'daemon 未连接' };
    }
    return new Promise((resolve, reject) => {
      this.pendingRequests.push({ resolve, reject });
      this.socket.write(JSON.stringify(msg) + '\n');
    });
  }

  // 订阅推送事件
  async subscribe(callback) {
    this.onPush = callback;
    const res = await this.request({ method: 'subscribe' });
    if (res.ok) this.subscribed = true;
    return res;
  }
}

// ==================== HTTP 服务 ====================

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.ico': 'image/x-icon',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
};

// 提供静态文件
function serveStatic(req, res) {
  let filePath = req.url.split('?')[0];
  if (filePath === '/') filePath = '/index.html';
  const fullPath = path.join(__dirname, 'public', filePath);
  // 安全检查：防止路径遍历
  if (!fullPath.startsWith(path.join(__dirname, 'public'))) {
    res.writeHead(403);
    res.end('Forbidden');
    return;
  }
  try {
    const data = fs.readFileSync(fullPath);
    const ext = path.extname(fullPath);
    res.writeHead(200, { 'Content-Type': MIME_TYPES[ext] || 'application/octet-stream' });
    res.end(data);
  } catch {
    res.writeHead(404);
    res.end('Not Found');
  }
}

// 读取请求体
function readBody(req) {
  return new Promise((resolve) => {
    let body = '';
    req.on('data', (chunk) => { body += chunk; });
    req.on('end', () => resolve(body));
  });
}

// 返回 JSON 响应
function jsonResponse(res, statusCode, data) {
  res.writeHead(statusCode, { 'Content-Type': 'application/json; charset=utf-8' });
  res.end(JSON.stringify(data));
}

// 读取当前用户信息
function readCurrentUser() {
  const mePath = path.join(opts.repo, '.gitim', 'me.json');
  try {
    const data = JSON.parse(fs.readFileSync(mePath, 'utf-8'));
    return { ok: true, data };
  } catch {
    return { ok: false, error: '无法读取 me.json' };
  }
}

// ==================== 启动 ====================

const daemon = new DaemonConnection(SOCKET_PATH);
const wsClients = new Set();

async function handleApi(req, res) {
  const url = new URL(req.url, `http://localhost:${opts.port}`);
  const pathname = url.pathname;

  // 当前用户（不需要 daemon）
  if (pathname === '/api/me' && req.method === 'GET') {
    const result = readCurrentUser();
    jsonResponse(res, result.ok ? 200 : 500, result);
    return;
  }

  // 以下 API 需要 daemon 连接
  if (!daemon.connected) {
    jsonResponse(res, 503, { ok: false, error: 'daemon 未连接' });
    return;
  }

  try {
    if (pathname === '/api/channels' && req.method === 'GET') {
      const result = await daemon.request({ method: 'channels' });
      jsonResponse(res, 200, result);
    } else if (pathname === '/api/users' && req.method === 'GET') {
      const result = await daemon.request({ method: 'users' });
      jsonResponse(res, 200, result);
    } else if (pathname === '/api/messages' && req.method === 'GET') {
      const channel = url.searchParams.get('channel');
      const limit = parseInt(url.searchParams.get('limit') || '50', 10);
      const since = parseInt(url.searchParams.get('since') || '0', 10);
      const result = await daemon.request({ method: 'read', channel, limit, since });
      jsonResponse(res, 200, result);
    } else if (pathname === '/api/thread' && req.method === 'GET') {
      const channel = url.searchParams.get('channel');
      const line = parseInt(url.searchParams.get('line') || '0', 10);
      const result = await daemon.request({ method: 'thread', channel, line_number: line });
      jsonResponse(res, 200, result);
    } else if (pathname === '/api/send' && req.method === 'POST') {
      const body = JSON.parse(await readBody(req));
      const result = await daemon.request({
        method: 'send',
        channel: body.channel,
        body: body.body,
        author: body.author,
        reply_to: body.reply_to || null,
      });
      jsonResponse(res, 200, result);
    } else if (pathname === '/api/status' && req.method === 'GET') {
      const result = await daemon.request({ method: 'status' });
      jsonResponse(res, 200, result);
    } else {
      jsonResponse(res, 404, { ok: false, error: '未知 API' });
    }
  } catch (err) {
    jsonResponse(res, 500, { ok: false, error: err.message });
  }
}

// 创建 HTTP 服务器
const server = http.createServer(async (req, res) => {
  // CORS 头
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type');

  if (req.method === 'OPTIONS') {
    res.writeHead(204);
    res.end();
    return;
  }

  if (req.url.startsWith('/api/')) {
    await handleApi(req, res);
  } else {
    serveStatic(req, res);
  }
});

// WebSocket 服务器
const wss = new WebSocketServer({ server, path: '/ws' });

wss.on('connection', (ws) => {
  wsClients.add(ws);
  console.log(`[ws] 客户端已连接，当前 ${wsClients.size} 个`);
  // 发送连接状态
  ws.send(JSON.stringify({ event: 'connected', daemon: daemon.connected }));
  ws.on('close', () => {
    wsClients.delete(ws);
    console.log(`[ws] 客户端断开，当前 ${wsClients.size} 个`);
  });
  ws.on('message', async (data) => {
    // 客户端也可以通过 WebSocket 发送消息
    try {
      const msg = JSON.parse(data.toString());
      if (msg.method === 'send' && daemon.connected) {
        const result = await daemon.request({
          method: 'send',
          channel: msg.channel,
          body: msg.body,
          author: msg.author,
          reply_to: msg.reply_to || null,
        });
        ws.send(JSON.stringify({ event: 'send_result', ...result }));
      }
    } catch (e) {
      ws.send(JSON.stringify({ event: 'error', error: e.message }));
    }
  });
});

// 广播推送事件给所有 WebSocket 客户端
function broadcastPush(event) {
  const msg = JSON.stringify(event);
  for (const ws of wsClients) {
    if (ws.readyState === 1) ws.send(msg);
  }
}

// 启动
async function main() {
  console.log(`[server] 仓库路径: ${opts.repo}`);
  console.log(`[server] Socket 路径: ${SOCKET_PATH}`);

  // 尝试连接 daemon
  const connected = await daemon.connect();
  if (connected) {
    // 订阅推送事件
    await daemon.subscribe((event) => {
      console.log('[daemon] 推送事件:', JSON.stringify(event));
      broadcastPush(event);
    });
    console.log('[daemon] 已订阅推送事件');
  } else {
    console.warn('[server] ⚠ daemon 未运行，HTTP API 将返回 503，前端可使用离线模式');
  }

  server.listen(opts.port, () => {
    console.log(`[server] WebUI 已启动: http://localhost:${opts.port}`);
  });
}

main().catch((err) => {
  console.error('[server] 启动失败:', err);
  process.exit(1);
});
