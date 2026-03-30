#!/usr/bin/env tsx
/**
 * runner.ts — 狼人杀 Game Runner
 *
 * 负责：
 *   1. 解析 CLI 参数（玩家数、daemon URL）
 *   2. 检查 daemon 是否在线
 *   3. 随机分配角色
 *   4. 注册用户 + 创建频道 + 发送角色 DM
 *   5. 启动 god-agent + player-agent 子进程
 *   6. 等待 god 退出后杀掉所有玩家进程
 */

import { spawn, type ChildProcess } from "node:child_process";
import { parseArgs } from "node:util";
import { callDaemon } from "./tools.js";
import { Role } from "./types.js";

// ── CLI Args ──────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    players: { type: "string", default: "5" },
    "daemon-url": { type: "string", default: process.env.GITIM_DAEMON_URL ?? "http://localhost:3000" },
  },
  strict: true,
});

const playerCount = parseInt(values.players!, 10);
if (isNaN(playerCount) || playerCount < 5 || playerCount > 7) {
  console.error("--players 必须在 5-7 之间");
  process.exit(1);
}

const daemonUrl = values["daemon-url"]!;

// Override env so callDaemon picks it up
process.env.GITIM_DAEMON_URL = daemonUrl;

// ── Constants ─────────────────────────────────────────────

const NAME_POOL = ["alice", "bob", "charlie", "dave", "eve", "frank", "grace"];

const DISPLAY_NAMES: Record<string, string> = {
  alice: "Alice",
  bob: "Bob",
  charlie: "Charlie",
  dave: "Dave",
  eve: "Eve",
  frank: "Frank",
  grace: "Grace",
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
  // Base roles for 5 players: 2 wolf, 1 seer, 1 witch, 1 villager
  const roles: Role[] = [Role.Wolf, Role.Wolf, Role.Seer, Role.Witch, Role.Villager];

  // 6+ players: add hunter, rest villager
  if (count >= 6) roles.push(Role.Hunter);
  for (let i = roles.length; i < count; i++) {
    roles.push(Role.Villager);
  }

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

// ── DM Channel Helper ─────────────────────────────────────

function dmChannel(a: string, b: string): string {
  return a <= b ? `dm:${a},${b}` : `dm:${b},${a}`;
}

// ── Daemon Health Check ───────────────────────────────────

async function checkDaemon(): Promise<void> {
  try {
    const res = await fetch(`${daemonUrl}/api`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ method: "status" }),
    });
    const json = await res.json();
    if (!json.ok) throw new Error(json.error ?? "status not ok");
    console.log("[runner] daemon 在线 ✓");
  } catch (err) {
    console.error(`[runner] 无法连接 daemon (${daemonUrl}):`, err);
    process.exit(1);
  }
}

// ── Register Users ────────────────────────────────────────

async function registerUser(handler: string, displayName: string): Promise<void> {
  await callDaemon({
    method: "register_user",
    handler,
    display_name: displayName,
  });
  console.log(`[runner] 注册用户: ${handler} (${displayName})`);
}

// ── Setup Channels & DMs ──────────────────────────────────

async function setupGame(players: PlayerAssignment[]): Promise<void> {
  const wolves = players.filter((p) => p.role === Role.Wolf);
  const wolfHandlers = wolves.map((w) => w.handler);

  // Register god
  await registerUser("god", "上帝");

  // Register all players
  for (const p of players) {
    await registerUser(p.handler, p.displayName);
  }

  // Create #general — god sends initial message
  await callDaemon({
    method: "send",
    channel: "general",
    body: "欢迎来到狼人杀！游戏即将开始。",
    author: "god",
  });
  console.log("[runner] 创建 #general 频道");

  // Create #wolves — god sends initial message, then wolves join
  await callDaemon({
    method: "send",
    channel: "wolves",
    body: "这是狼人专属频道。你们可以在这里密谋。",
    author: "god",
  });
  for (const w of wolves) {
    await callDaemon({
      method: "join_channel",
      channel: "wolves",
      handler: w.handler,
    });
  }
  console.log(`[runner] 创建 #wolves 频道 (${wolfHandlers.join(", ")})`);

  // Send role assignment DMs from god to each player
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
    await callDaemon({
      method: "send",
      channel: dm,
      body: roleMsg,
      author: "god",
    });
    console.log(`[runner] DM → ${p.handler}: 身份通知已发送`);
  }
}

// ── Spawn Processes ───────────────────────────────────────

function spawnGod(players: PlayerAssignment[]): ChildProcess {
  const playersArg = players.map((p) => `${p.handler}:${p.role}`).join(",");
  const args = [
    "src/god-agent.ts",
    "--players", playersArg,
    "--daemon-url", daemonUrl,
  ];

  console.log(`[runner] 启动 god-agent`);
  return spawn("npx", ["tsx", ...args], {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: { ...process.env, GITIM_DAEMON_URL: daemonUrl },
  });
}

function spawnPlayer(p: PlayerAssignment, wolves: string[]): ChildProcess {
  const wolfPartners = p.role === Role.Wolf
    ? wolves.filter((h) => h !== p.handler)
    : [];

  const args = [
    "src/player-agent.ts",
    "--handler", p.handler,
    "--role", p.role,
    "--display-name", p.displayName,
    "--personality", p.personality,
    "--daemon-url", daemonUrl,
    "--wolves", wolves.join(","),
  ];
  if (wolfPartners.length > 0) {
    args.push("--wolf-partners", wolfPartners.join(","));
  }

  console.log(`[runner] 启动 player-agent: ${p.handler} (${p.role})`);
  return spawn("npx", ["tsx", ...args], {
    stdio: ["ignore", "inherit", "inherit"],
    cwd: process.cwd(),
    env: { ...process.env, GITIM_DAEMON_URL: daemonUrl },
  });
}

// ── Main ──────────────────────────────────────────────────

async function main(): Promise<void> {
  console.log(`[runner] 狼人杀 Game Runner 启动`);
  console.log(`[runner] 玩家数: ${playerCount}, daemon: ${daemonUrl}`);

  // 1. Check daemon
  await checkDaemon();

  // 2. Assign roles
  const players = assignRoles(playerCount);
  const wolves = players.filter((p) => p.role === Role.Wolf).map((p) => p.handler);

  console.log("[runner] 角色分配:");
  for (const p of players) {
    console.log(`  ${p.handler} → ${p.role} (${p.personality.slice(0, 15)}...)`);
  }

  // 3. Setup game: register users, create channels, send DMs
  await setupGame(players);

  // 4. Spawn god + players
  const children: ChildProcess[] = [];

  const godProc = spawnGod(players);
  children.push(godProc);

  // Small delay before spawning players so god can initialize
  await new Promise((r) => setTimeout(r, 1000));

  for (const p of players) {
    const proc = spawnPlayer(p, wolves);
    children.push(proc);
  }

  // 5. SIGINT handler: kill all children
  const cleanup = () => {
    console.log("\n[runner] 收到终止信号，清理子进程...");
    for (const child of children) {
      if (!child.killed) {
        child.kill("SIGTERM");
      }
    }
    // Give a moment for graceful shutdown, then force
    setTimeout(() => {
      for (const child of children) {
        if (!child.killed) {
          child.kill("SIGKILL");
        }
      }
      process.exit(0);
    }, 3000);
  };

  process.on("SIGINT", cleanup);
  process.on("SIGTERM", cleanup);

  // 6. Wait for god to exit → kill remaining players
  godProc.on("exit", (code) => {
    console.log(`[runner] god-agent 退出 (code=${code})，终止所有玩家进程...`);
    for (const child of children) {
      if (child !== godProc && !child.killed) {
        child.kill("SIGTERM");
      }
    }
    setTimeout(() => process.exit(code ?? 0), 2000);
  });

  // Keep alive — the process exits via godProc.on('exit') or SIGINT
  await new Promise(() => {});
}

main().catch((err) => {
  console.error("[runner] Fatal:", err);
  process.exit(1);
});
