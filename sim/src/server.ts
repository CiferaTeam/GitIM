import http from "node:http";
import { openDb } from "./db.js";
import { handleRequest } from "./handlers.js";

// ── Mock GitIM Daemon ───────────────────────────────────────
//
//  和 gitim-daemon 的 HTTP 模式完全对齐：
//    POST /api       → JSON { method, ...params } → Response
//    GET  /api/events → SSE (P2, 暂不实现)
//
//  用法: tsx src/server.ts [--port 3000] [--db path/to/db.sqlite]

const args = process.argv.slice(2);
const portIdx = args.indexOf("--port");
const dbIdx = args.indexOf("--db");
const PORT = portIdx >= 0 ? parseInt(args[portIdx + 1], 10) : 3000;
const DB_PATH = dbIdx >= 0 ? args[dbIdx + 1] : undefined;

const { db, close } = openDb(DB_PATH);

const server = http.createServer(async (req, res) => {
  // CORS headers for local development
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "POST, GET, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type");

  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  if (req.method === "POST" && req.url === "/api") {
    let body = "";
    for await (const chunk of req) body += chunk;

    try {
      const parsed = JSON.parse(body);
      const response = handleRequest(db, parsed);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(response));
    } catch {
      res.writeHead(400, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: false, error: "invalid JSON" }));
    }
    return;
  }

  if (req.method === "GET" && req.url === "/api/events") {
    // P2: SSE endpoint stub
    res.writeHead(501, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ ok: false, error: "SSE not implemented in P1" }));
    return;
  }

  res.writeHead(404, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ ok: false, error: "not found" }));
});

server.listen(PORT, () => {
  console.log(`[mock-daemon] GitIM mock daemon running on http://localhost:${PORT}`);
  console.log(`[mock-daemon] DB: ${DB_PATH ?? "gitim-sim.db"}`);
  console.log(`[mock-daemon] POST /api — GitIM API`);
});

process.on("SIGINT", () => {
  console.log("\n[mock-daemon] shutting down...");
  close();
  server.close();
  process.exit(0);
});

process.on("SIGTERM", () => {
  close();
  server.close();
  process.exit(0);
});
