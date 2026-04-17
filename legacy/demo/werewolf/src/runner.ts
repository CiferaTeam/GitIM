#!/usr/bin/env tsx
/**
 * runner.ts — 狼人杀 Game Runner（配置驱动）
 *
 * 读取 JSON 配置文件，验证角色合法性，
 * 为每个 agent 启动独立的 git clone + daemon + socket。
 * Runner 只做基础设施（注册用户），游戏逻辑全由 God 通过 IM 驱动。
 *
 * 启动顺序：所有 daemon+onboard → 所有 player → 最后 God
 */

import { spawn, execSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { callDaemon } from "./tools.js";
import { Role } from "./types.js";

// ── CLI Args ──────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    config: { type: "string" },
    "game-id": { type: "string" },
  },
  strict: true,
});

if (!values.config) {
  console.error("用法: npm start -- --config <path-to-config.json> [--game-id N]");
  process.exit(1);
}

const gameId = parseInt(values["game-id"] ?? "1", 10);
if (isNaN(gameId) || gameId < 1) {
  console.error("game-id 必须为正整数");
  process.exit(1);
}

// ── Config Schema ─────────────────────────────────────────

interface PlayerConfig {
  role: string;
  personality?: string;
}

interface GameConfig {
  work_dir: string;
  players: Record<string, PlayerConfig>;
}

// ── Constants ─────────────────────────────────────────────

const PERSONALITY_POOL = [
  "你性格沉稳冷静，善于观察细节，发言总是有理有据。",
  "你是个话痨，喜欢不停发言和提问，经常打断别人的思路。",
  "你很有攻击性，喜欢直接质疑别人，不怕得罪人。",
  "你表面温和友善，但内心精明，擅长套话和引导讨论。",
  "你是个逻辑怪，喜欢用排除法和概率分析来推理。",
  "你是个演技派，说话时喜欢带入角色，表现得非常真诚。",
  "你比较安静内向，但一旦发言必有关键信息，善于在关键时刻一击致命。",
  "你是老好人，总想调和矛盾，但关键时刻会坚定站队。",
  "你疑心很重，对所有人都保持警惕，喜欢反复确认信息。",
  "你很果断，一旦形成判断就坚持到底，不轻易被人说服。",
  "你是个跟风派，喜欢观察多数人的意见再做决定，但偶尔会有自己的独到见解。",
  "你很幽默，喜欢用玩笑缓解紧张气氛，但推理的时候非常认真。",
];

// ── Load & Validate Config ────────────────────────────────

function loadConfig(configPath: string): GameConfig {
  const raw = fs.readFileSync(configPath, "utf-8");
  const config: GameConfig = JSON.parse(raw);

  if (!config.work_dir) throw new Error("config 缺少 work_dir");
  if (!config.players || typeof config.players !== "object") throw new Error("config 缺少 players");

  const handlers = Object.keys(config.players);
  if (handlers.length < 5 || handlers.length > 12) {
    throw new Error(`玩家数必须在 5-12 之间，当前 ${handlers.length}`);
  }

  const validRoles: Set<string> = new Set(Object.values(Role).filter((r) => r !== Role.God));
  const roleCounts: Record<string, number> = {};

  for (const [handler, pc] of Object.entries(config.players)) {
    if (!validRoles.has(pc.role as Role)) {
      throw new Error(`玩家 ${handler} 的角色 "${pc.role}" 无效。可选: ${[...validRoles].join(", ")}`);
    }
    roleCounts[pc.role] = (roleCounts[pc.role] ?? 0) + 1;
  }

  // Validate basic role composition
  if ((roleCounts["wolf"] ?? 0) < 2) throw new Error("至少需要 2 个狼人");
  if ((roleCounts["seer"] ?? 0) !== 1) throw new Error("必须恰好 1 个预言家");
  if ((roleCounts["witch"] ?? 0) !== 1) throw new Error("必须恰好 1 个女巫");

  return config;
}

// ── Resolve Players ───────────────────────────────────────

interface ResolvedPlayer {
  handler: string;
  displayName: string;
  role: Role;
  personality: string;
}

function resolvePlayers(players: Record<string, PlayerConfig>): ResolvedPlayer[] {
  // Shuffle personality pool for random assignment
  const pool = [...PERSONALITY_POOL];
  for (let i = pool.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [pool[i], pool[j]] = [pool[j], pool[i]];
  }

  let poolIdx = 0;
  return Object.entries(players).map(([handler, pc]) => ({
    handler,
    displayName: handler.charAt(0).toUpperCase() + handler.slice(1),
    role: pc.role as Role,
    personality: pc.personality ?? pool[poolIdx++ % pool.length],
  }));
}

// ── Daemon Lifecycle ──────────────────────────────────────

function socketPathFor(repoDir: string): string {
  return path.join(repoDir, ".gitim", "run", "gitim.sock");
}

async function startDaemon(repoDir: string): Promise<ChildProcess> {
  const runDir = path.join(repoDir, ".gitim", "run");
  fs.mkdirSync(runDir, { recursive: true });
  for (const f of ["gitim.pid", "gitim.sock", "gitim.port", "gitim.lock"]) {
    try { fs.unlinkSync(path.join(runDir, f)); } catch { /* ignore */ }
  }

  const child = spawn("gitim-daemon", [], {
    cwd: repoDir,
    detached: true,
    stdio: ["ignore", "ignore", "ignore"],
  });
  child.unref();

  const sockPath = socketPathFor(repoDir);
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return child;
    await sleep(100);
  }
  throw new Error(`daemon failed to start in ${repoDir}`);
}

async function onboard(socketPath: string, handler: string, displayName: string): Promise<void> {
  for (let attempt = 0; attempt < 5; attempt++) {
    try {
      await callDaemon(socketPath, {
        method: "onboard",
        git_server: "git",
        auth: { handler, display_name: displayName },
      });
      return;
    } catch (err) {
      if (attempt < 4) {
        console.log(`[runner] ${handler} onboard 重试 (${attempt + 1}/5): ${err instanceof Error ? err.message : err}`);
        await sleep(2000);
      } else {
        throw err;
      }
    }
  }
}

// ── Git Setup ─────────────────────────────────────────────

function exec(cmd: string, cwd: string): string {
  return execSync(cmd, { cwd, encoding: "utf-8", stdio: ["pipe", "pipe", "pipe"] }).trim();
}

function setupBareRepo(dir: string): void {
  fs.mkdirSync(dir, { recursive: true });
  exec("git init --bare", dir);
}

function setupFirstRepo(repoDir: string, remoteUrl: string): void {
  fs.mkdirSync(repoDir, { recursive: true });
  exec("git init", repoDir);
  exec(`git remote add origin ${remoteUrl}`, repoDir);
  fs.mkdirSync(path.join(repoDir, ".gitim"), { recursive: true });
}

function cloneRepo(remoteUrl: string, repoDir: string): void {
  execSync(`git clone ${remoteUrl} ${repoDir}`, { encoding: "utf-8", stdio: ["pipe", "pipe", "pipe"] });
  fs.mkdirSync(path.join(repoDir, ".gitim"), { recursive: true });
}

// ── Spawn Agent Processes ─────────────────────────────────

function spawnGod(socketPath: string, repoDir: string, players: ResolvedPlayer[], gid: number): ChildProcess {
  const playersArg = players.map((p) => `${p.handler}:${p.role}`).join(",");
  return spawn("npx", [
    "tsx", "src/god-agent.ts",
    "--players", playersArg,
    "--socket-path", socketPath,
    "--repo-dir", repoDir,
    "--game-id", String(gid),
  ], {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: process.env,
  });
}

function spawnPlayer(p: ResolvedPlayer, socketPath: string, repoDir: string, gid: number): ChildProcess {
  return spawn("npx", [
    "tsx", "src/player-agent.ts",
    "--handler", p.handler,
    "--display-name", p.displayName,
    "--personality", p.personality,
    "--socket-path", socketPath,
    "--repo-dir", repoDir,
    "--game-id", String(gid),
  ], {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: process.env,
  });
}

// ── Main ──────────────────────────────────────────────────

async function main(): Promise<void> {
  const config = loadConfig(values.config!);
  const players = resolvePlayers(config.players);
  const workDir = config.work_dir;

  fs.mkdirSync(workDir, { recursive: true });

  const bareDir = path.join(workDir, "remote.git");
  const remoteUrl = `file://${bareDir}`;
  const godDir = path.join(workDir, "god");
  const godSocket = socketPathFor(godDir);

  console.log(`[runner] 狼人杀 Game Runner 启动 (game-id: ${gameId})`);
  console.log(`[runner] 玩家数: ${players.length}, 工作目录: ${workDir}`);

  // Print role assignments
  console.log("[runner] 角色分配:");
  for (const p of players) {
    console.log(`  ${p.handler} → ${p.role}`);
  }

  const daemonProcs: ChildProcess[] = [];
  const agentProcs: ChildProcess[] = [];
  const playerSockets: Record<string, string> = {};

  // Check if infrastructure already exists (reuse mode)
  const reuse = fs.existsSync(bareDir) && fs.existsSync(godDir);

  if (reuse) {
    console.log("[runner] 检测到已有基础设施，进入复用模式");

    // Ensure daemons are running, start if not
    const ensureDaemon = async (repoDir: string, label: string): Promise<void> => {
      const sock = socketPathFor(repoDir);
      try {
        await callDaemon(sock, { method: "poll", since: null });
        console.log(`[runner] ${label} daemon 已在运行`);
      } catch {
        console.log(`[runner] ${label} daemon 未运行，启动中...`);
        const daemon = await startDaemon(repoDir);
        daemonProcs.push(daemon);
        console.log(`[runner] ${label} daemon 已启动`);
      }
    };

    await ensureDaemon(godDir, "god");
    for (const p of players) {
      const playerDir = path.join(workDir, p.handler);
      if (!fs.existsSync(playerDir)) {
        // New player — clone and onboard
        console.log(`[runner] ${p.handler} 是新玩家，创建 repo...`);
        cloneRepo(remoteUrl, playerDir);
        const daemon = await startDaemon(playerDir);
        daemonProcs.push(daemon);
        const sock = socketPathFor(playerDir);
        await onboard(sock, p.handler, p.displayName);
        playerSockets[p.handler] = sock;
        console.log(`[runner] ${p.handler} daemon 就绪`);
      } else {
        await ensureDaemon(playerDir, p.handler);
        playerSockets[p.handler] = socketPathFor(playerDir);
      }
    }
  } else {
    // Fresh setup
    setupBareRepo(bareDir);
    console.log(`[runner] 创建 bare repo: ${bareDir}`);

    setupFirstRepo(godDir, remoteUrl);
    const godDaemon = await startDaemon(godDir);
    daemonProcs.push(godDaemon);
    await onboard(godSocket, "god", "上帝");
    console.log("[runner] God daemon 就绪");

    for (const p of players) {
      const playerDir = path.join(workDir, p.handler);
      cloneRepo(remoteUrl, playerDir);
      const daemon = await startDaemon(playerDir);
      daemonProcs.push(daemon);
      const sock = socketPathFor(playerDir);
      await onboard(sock, p.handler, p.displayName);
      playerSockets[p.handler] = sock;
      console.log(`[runner] ${p.handler} daemon 就绪`);
    }
  }

  // Register all players on God's daemon (idempotent)
  for (const p of players) {
    try {
      await callDaemon(godSocket, {
        method: "register_user",
        handler: p.handler,
        display_name: p.displayName,
      });
    } catch { /* idempotent, ignore */ }
  }
  console.log(`[runner] God daemon 注册 ${players.length} 名玩家`);

  // Spawn all player agents
  for (const p of players) {
    const playerDir = path.join(workDir, p.handler);
    const proc = spawnPlayer(p, playerSockets[p.handler], playerDir, gameId);
    agentProcs.push(proc);
  }
  console.log("[runner] 所有 player agent 已启动");

  await sleep(1000);

  // Spawn God agent LAST
  const godProc = spawnGod(godSocket, godDir, players, gameId);
  agentProcs.push(godProc);
  console.log(`[runner] God agent 已启动，第 ${gameId} 局游戏开始`);

  // 7. Cleanup handler
  const cleanup = () => {
    console.log("\n[runner] 清理中...");
    for (const child of agentProcs) {
      if (!child.killed) child.kill("SIGTERM");
    }
    setTimeout(() => {
      for (const child of [...agentProcs, ...daemonProcs]) {
        if (!child.killed) child.kill("SIGKILL");
      }
      process.exit(0);
    }, 3000);
  };

  process.on("SIGINT", cleanup);
  process.on("SIGTERM", cleanup);

  // 8. Wait for god to exit → cleanup
  godProc.on("exit", (code) => {
    console.log(`[runner] god-agent 退出 (code=${code})，终止所有进程...`);
    for (const child of agentProcs) {
      if (child !== godProc && !child.killed) child.kill("SIGTERM");
    }
    setTimeout(() => {
      for (const d of daemonProcs) {
        if (!d.killed) d.kill("SIGTERM");
      }
      console.log(`[runner] 游戏数据保存在: ${workDir}`);
      process.exit(code ?? 0);
    }, 2000);
  });

  await new Promise(() => {});
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

main().catch((err) => {
  console.error("[runner] Fatal:", err);
  process.exit(1);
});
