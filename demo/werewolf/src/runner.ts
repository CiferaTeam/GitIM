#!/usr/bin/env tsx
/**
 * runner.ts — 狼人杀 Game Runner
 *
 * 每个 agent 独立 daemon + 独立 git clone + socket 通信。
 *
 * 流程：
 *   1. 创建 bare git repo 作为共享 remote
 *   2. God 先 init + onboard（创建频道、用户）
 *   3. 玩家依次 clone + onboard
 *   4. God 设置游戏（角色 DM、频道成员）
 *   5. 启动 god-agent + player-agent 子进程
 *   6. 等待 god 退出后清理
 */

import { spawn, execSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { callDaemon } from "./tools.js";
import { Role, dmChannel } from "./types.js";

// ── CLI Args ──────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    players: { type: "string", default: "5" },
    "work-dir": { type: "string", default: "" },
  },
  strict: true,
});

const playerCount = parseInt(values.players!, 10);
if (isNaN(playerCount) || playerCount < 5 || playerCount > 7) {
  console.error("--players 必须在 5-7 之间");
  process.exit(1);
}

// ── Constants ─────────────────────────────────────────────

const NAME_POOL = ["alice", "bob", "charlie", "dave", "eve", "frank", "grace"];

const DISPLAY_NAMES: Record<string, string> = {
  alice: "Alice", bob: "Bob", charlie: "Charlie", dave: "Dave",
  eve: "Eve", frank: "Frank", grace: "Grace",
};

const PERSONALITY_POOL = [
  "你性格沉稳冷静，善于观察细节，发言总是有理有据。",
  "你是个话痨，喜欢不停发言和提问，经常打断别人的思路。",
  "你很有攻击性，喜欢直接质疑别人，不怕得罪人。",
  "你表面温和友善，但内心精明，擅长套话和引导讨论。",
  "你是个逻辑怪，喜欢用排除法和概率分析来推理。",
  "你是个演技派，说话时喜欢带入角色，表现得非常真诚。",
  "你比较安静内向，但一旦发言必有关键信息，善于在关键时刻一击致命。",
];

// ── Role Assignment ───────────────────────────────────────

interface PlayerAssignment {
  handler: string;
  displayName: string;
  role: Role;
  personality: string;
}

function assignRoles(count: number): PlayerAssignment[] {
  const roles: Role[] = [Role.Wolf, Role.Wolf, Role.Seer, Role.Witch, Role.Villager];
  if (count >= 6) roles.push(Role.Hunter);
  for (let i = roles.length; i < count; i++) roles.push(Role.Villager);

  // Shuffle roles
  for (let i = roles.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [roles[i], roles[j]] = [roles[j], roles[i]];
  }

  // Shuffle personalities
  const personalities = [...PERSONALITY_POOL];
  for (let i = personalities.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [personalities[i], personalities[j]] = [personalities[j], personalities[i]];
  }

  const names = NAME_POOL.slice(0, count);
  return names.map((name, i) => ({
    handler: name,
    displayName: DISPLAY_NAMES[name],
    role: roles[i],
    personality: personalities[i],
  }));
}

// ── Daemon Lifecycle ──────────────────────────────────────

function socketPathFor(repoDir: string): string {
  return path.join(repoDir, ".gitim", "run", "gitim.sock");
}

async function startDaemon(repoDir: string): Promise<ChildProcess> {
  // Clean stale runtime files
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

  // Wait for socket to appear
  const sockPath = socketPathFor(repoDir);
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return child;
    await sleep(100);
  }
  throw new Error(`daemon failed to start in ${repoDir}`);
}

async function onboard(socketPath: string, handler: string, displayName: string): Promise<void> {
  await callDaemon(socketPath, {
    method: "onboard",
    git_server: "git",
    auth: { handler, display_name: displayName },
  });
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

// ── Game Setup ────────────────────────────────────────────

async function setupGame(
  godSocket: string,
  players: PlayerAssignment[]
): Promise<void> {
  const wolves = players.filter((p) => p.role === Role.Wolf);
  const wolfHandlers = wolves.map((w) => w.handler);

  // Create #general
  await callDaemon(godSocket, {
    method: "send",
    channel: "general",
    body: "欢迎来到狼人杀！游戏即将开始。",
    author: "god",
  });
  console.log("[runner] 创建 #general 频道");

  // Create #wolves + add wolf members
  await callDaemon(godSocket, {
    method: "send",
    channel: "wolves",
    body: "这是狼人专属频道。你们可以在这里密谋。",
    author: "god",
  });
  for (const w of wolves) {
    await callDaemon(godSocket, {
      method: "join_channel",
      channel: "wolves",
      handler: w.handler,
    });
  }
  console.log(`[runner] 创建 #wolves 频道 (${wolfHandlers.join(", ")})`);

  // Send role assignment DMs
  for (const p of players) {
    const dm = dmChannel("god", p.handler);
    let roleMsg: string;
    switch (p.role) {
      case Role.Wolf:
        roleMsg = `你的身份是【狼人】。你的同伴是: ${wolfHandlers.filter((h) => h !== p.handler).join("、")}。在 #wolves 频道与同伴沟通。`;
        break;
      case Role.Seer:
        roleMsg = "你的身份是【预言家】。每晚你可以查验一名玩家的身份。";
        break;
      case Role.Witch:
        roleMsg = "你的身份是【女巫】。你有一瓶解药和一瓶毒药，各限用一次。";
        break;
      case Role.Hunter:
        roleMsg = "你的身份是【猎人】。被淘汰时你可以开枪带走一名玩家。";
        break;
      case Role.Villager:
        roleMsg = "你的身份是【村民】。你没有特殊技能，但你的观察力和投票权是关键。";
        break;
      default:
        roleMsg = `你的身份是【${p.role}】。`;
    }
    await callDaemon(godSocket, { method: "send", channel: dm, body: roleMsg, author: "god" });
    console.log(`[runner] DM → ${p.handler}: 身份通知已发送`);
  }
}

// ── Spawn Agent Processes ─────────────────────────────────

function spawnGod(socketPath: string, players: PlayerAssignment[]): ChildProcess {
  const playersArg = players.map((p) => `${p.handler}:${p.role}`).join(",");
  return spawn("npx", ["tsx", "src/god-agent.ts", "--players", playersArg, "--socket-path", socketPath], {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: process.env,
  });
}

function spawnPlayer(p: PlayerAssignment, socketPath: string, wolves: string[]): ChildProcess {
  const wolfPartners = p.role === Role.Wolf ? wolves.filter((h) => h !== p.handler) : [];
  const args = [
    "tsx", "src/player-agent.ts",
    "--handler", p.handler,
    "--role", p.role,
    "--display-name", p.displayName,
    "--personality", p.personality,
    "--socket-path", socketPath,
  ];
  if (wolfPartners.length > 0) {
    args.push("--wolf-partners", wolfPartners.join(","));
  }
  return spawn("npx", args, {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: process.env,
  });
}

// ── Main ──────────────────────────────────────────────────

async function main(): Promise<void> {
  // Work directory
  const workDir = values["work-dir"]
    || path.join(process.env.TMPDIR ?? "/tmp", `werewolf-${Date.now()}`);
  fs.mkdirSync(workDir, { recursive: true });

  const bareDir = path.join(workDir, "remote.git");
  const remoteUrl = `file://${bareDir}`;

  console.log(`[runner] 狼人杀 Game Runner 启动`);
  console.log(`[runner] 玩家数: ${playerCount}, 工作目录: ${workDir}`);

  // 1. Create bare repo
  setupBareRepo(bareDir);
  console.log(`[runner] 创建 bare repo: ${bareDir}`);

  // 2. Assign roles
  const players = assignRoles(playerCount);
  const wolves = players.filter((p) => p.role === Role.Wolf).map((p) => p.handler);

  console.log("[runner] 角色分配:");
  for (const p of players) {
    console.log(`  ${p.handler} → ${p.role} (${p.personality.slice(0, 15)}...)`);
  }

  // 3. Setup God: init → daemon → onboard
  const godDir = path.join(workDir, "god");
  setupFirstRepo(godDir, remoteUrl);
  const godDaemon = await startDaemon(godDir);
  const godSocket = socketPathFor(godDir);
  await onboard(godSocket, "god", "上帝");
  console.log("[runner] God daemon 就绪");

  // Track all daemon processes for cleanup
  const daemonProcs: ChildProcess[] = [godDaemon];
  const agentProcs: ChildProcess[] = [];

  // 4. Setup each player: clone → daemon → onboard
  const playerSockets: Record<string, string> = {};
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

  // 5. God creates game channels and sends role DMs
  await setupGame(godSocket, players);

  // 6. Spawn agent processes
  const godProc = spawnGod(godSocket, players);
  agentProcs.push(godProc);

  await sleep(1000);

  for (const p of players) {
    const proc = spawnPlayer(p, playerSockets[p.handler], wolves);
    agentProcs.push(proc);
  }

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
