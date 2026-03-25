import http from 'node:http';
import fs from 'node:fs/promises';
import { existsSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { execSync } from 'node:child_process';
import { GitimClient } from '../client.js';

// Module-level client, initialized in startServer
let client: GitimClient;
let repoRoot: string;

// ---------- helpers ----------

function jsonResponse(res: http.ServerResponse, status: number, body: unknown): void {
  const json = JSON.stringify(body);
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(json),
  });
  res.end(json);
}

function parseQuery(url: string): URLSearchParams {
  const idx = url.indexOf('?');
  return new URLSearchParams(idx >= 0 ? url.slice(idx + 1) : '');
}

const MAX_BODY_BYTES = 64 * 1024;

async function readBody(req: http.IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of req) {
    total += (chunk as Buffer).length;
    if (total > MAX_BODY_BYTES) throw new Error('Request body too large');
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks).toString('utf-8');
}

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
};

// ---------- /api/me ----------

async function handleMe(res: http.ServerResponse): Promise<void> {
  const meJsonPath = path.join(repoRoot, '.gitim', 'me.json');
  try {
    const raw = await fs.readFile(meJsonPath, 'utf-8');
    const me = JSON.parse(raw);
    jsonResponse(res, 200, { ok: true, data: me });
  } catch {
    try {
      const name = execSync('git config user.name', { cwd: repoRoot, encoding: 'utf-8' }).trim();
      jsonResponse(res, 200, { ok: true, data: { handler: name.toLowerCase().replace(/\s+/g, '-'), display_name: name } });
    } catch {
      jsonResponse(res, 500, { ok: false, error: 'Cannot determine identity: no me.json and git config user.name not set' });
    }
  }
}

// ---------- API routing ----------

async function handleApi(req: http.IncomingMessage, res: http.ServerResponse): Promise<void> {
  const url = req.url ?? '/';
  const pathname = url.split('?')[0];
  const query = parseQuery(url);

  try {
    if (pathname === '/api/me' && req.method === 'GET') {
      await handleMe(res);
      return;
    }

    if (pathname === '/api/poll' && req.method === 'GET') {
      const since = query.get('since') ?? undefined;
      const result = await client.poll(since);
      jsonResponse(res, 200, result);
      return;
    }

    if (pathname === '/api/channels' && req.method === 'GET') {
      const result = await client.listChannels();
      jsonResponse(res, 200, result);
      return;
    }

    if (pathname === '/api/users' && req.method === 'GET') {
      const result = await client.listUsers();
      jsonResponse(res, 200, result);
      return;
    }

    if (pathname === '/api/read' && req.method === 'GET') {
      const channel = query.get('channel');
      if (!channel) {
        jsonResponse(res, 400, { ok: false, error: 'Missing required parameter: channel' });
        return;
      }
      const limitStr = query.get('limit');
      const limit = limitStr ? parseInt(limitStr, 10) : undefined;
      const result = await client.read(channel, limit);
      jsonResponse(res, 200, result);
      return;
    }

    if (pathname === '/api/thread' && req.method === 'GET') {
      const channel = query.get('channel');
      const lineStr = query.get('line');
      if (!channel || !lineStr) {
        jsonResponse(res, 400, { ok: false, error: 'Missing required parameters: channel, line' });
        return;
      }
      const line = parseInt(lineStr, 10);
      const result = await client.getThread(channel, line);
      jsonResponse(res, 200, result);
      return;
    }

    if (pathname === '/api/send' && req.method === 'POST') {
      const raw = await readBody(req);
      let body: { channel?: string; body?: string; author?: string; reply_to?: number };
      try {
        body = JSON.parse(raw);
      } catch {
        jsonResponse(res, 400, { ok: false, error: 'Invalid JSON body' });
        return;
      }
      if (!body.channel || !body.body) {
        jsonResponse(res, 400, { ok: false, error: 'Missing required fields: channel, body' });
        return;
      }
      const result = await client.send(body.channel, body.body, body.author, body.reply_to);
      jsonResponse(res, 200, result);
      return;
    }

    jsonResponse(res, 404, { ok: false, error: `Unknown API endpoint: ${pathname}` });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    jsonResponse(res, 502, { ok: false, error: `Daemon error: ${message}` });
  }
}

// ---------- Static file serving (production) ----------

function serveStatic(req: http.IncomingMessage, res: http.ServerResponse, staticDir: string): void {
  const url = req.url ?? '/';
  const pathname = url.split('?')[0];

  // Resolve and guard against path traversal
  const requestedPath = path.resolve(staticDir, '.' + pathname);
  if (!requestedPath.startsWith(staticDir)) {
    res.writeHead(403);
    res.end('Forbidden');
    return;
  }

  let filePath = requestedPath;

  // If requesting a directory, try index.html
  if (filePath.endsWith('/') || !path.extname(filePath)) {
    const asIndex = path.join(filePath, 'index.html');
    if (existsSync(asIndex)) {
      filePath = asIndex;
    }
  }

  // Try to serve the file; SPA fallback to index.html if not found
  if (!existsSync(filePath) || statSync(filePath).isDirectory()) {
    filePath = path.join(staticDir, 'index.html');
    if (!existsSync(filePath)) {
      res.writeHead(404);
      res.end('Not Found');
      return;
    }
  }

  const ext = path.extname(filePath);
  const contentType = MIME_TYPES[ext] ?? 'application/octet-stream';
  const content = readFileSync(filePath);
  res.writeHead(200, {
    'Content-Type': contentType,
    'Content-Length': content.length,
  });
  res.end(content);
}

// ---------- startServer ----------

export interface ServerOptions {
  repoRoot: string;
  port: number;
  dev: boolean;
}

export async function startServer(options: ServerOptions): Promise<http.Server> {
  repoRoot = options.repoRoot;
  client = new GitimClient(repoRoot);

  const staticDir = path.resolve(import.meta.dirname, '../../dist/webui');
  const viteRoot = path.resolve(import.meta.dirname, '../../../webui');

  // In dev mode, dynamically import vite and create middleware-mode server.
  // Use string variable for import() to bypass TypeScript module resolution —
  // vite is only a devDependency of the webui package, not of cli.
  let vite: { middlewares: http.RequestListener } | undefined;
  if (options.dev) {
    const vitePkg = 'vite';
    const { createServer: createViteServer } = await import(/* webpackIgnore: true */ vitePkg);
    vite = await createViteServer({
      root: viteRoot,
      server: { middlewareMode: true },
    });
  }

  const server = http.createServer(async (req, res) => {
    const url = req.url ?? '/';

    // API routes
    if (url.startsWith('/api/')) {
      await handleApi(req, res);
      return;
    }

    // Dev mode: proxy through Vite
    if (vite) {
      (vite.middlewares as any)(req, res);
      return;
    }

    // Production: serve static files
    serveStatic(req, res, staticDir);
  });

  return new Promise((resolve, reject) => {
    server.on('error', (err: NodeJS.ErrnoException) => {
      if (err.code === 'EADDRINUSE') {
        reject(new Error(`Port ${options.port} is already in use`));
      } else {
        reject(err);
      }
    });

    server.listen(options.port, '127.0.0.1', () => {
      console.log(`GitIM WebUI server listening on http://localhost:${options.port}`);
      if (options.dev) {
        console.log('  Mode: development (Vite HMR enabled)');
      } else {
        console.log('  Mode: production (serving static files)');
      }
      resolve(server);
    });
  });
}
