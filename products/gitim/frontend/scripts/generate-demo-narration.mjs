/**
 * Offline MiMo TTS pipeline for the landing demo narration.
 *
 * Reads every frame's `narration` from src/lib/demo-story/scenario.ts,
 * synthesizes it with the MiMo TTS chat-completions endpoint, and writes:
 *   public/demo-narration/<frame-id>.mp3
 *   src/lib/demo-story/narration-manifest.json  (frame-id → durationMs)
 *
 * Usage:
 *   MIMO_API_KEY=... npm run generate:narration
 *
 * Idempotent: frames whose .mp3 already exists are skipped (delete the file
 * to regenerate). The API key is never written into the repo. Requires
 * ffprobe on PATH (ffmpeg) to measure clip durations.
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FRONTEND = path.resolve(HERE, "..");
const SCENARIO = path.join(FRONTEND, "src/lib/demo-story/scenario.ts");
const OUT_DIR = path.join(FRONTEND, "public/demo-narration");
const MANIFEST = path.join(FRONTEND, "src/lib/demo-story/narration-manifest.json");

const API_URL = "https://token-plan-cn.xiaomimimo.com/v1/chat/completions";
const MODEL = "mimo-v2.5-tts";
const VOICE = "Chloe";
const STYLE =
  "Read this aloud in a calm, confident product-demo narration tone. " +
  "Moderate pace, clear articulation, no dramatization.";

function extractNarrations(source) {
  const frames = [];
  // Split on frame( { id: "..." boundaries, then find narration inside each.
  const idRe = /frame\(\{\s*id:\s*"([^"]+)"/g;
  const bounds = [];
  let m;
  while ((m = idRe.exec(source)) !== null) {
    bounds.push({ id: m[1], start: m.index });
  }
  for (let i = 0; i < bounds.length; i += 1) {
    const end = i + 1 < bounds.length ? bounds[i + 1].start : source.length;
    const block = source.slice(bounds[i].start, end);
    const nm = /narration:\s*"((?:[^"\\]|\\.)*)"/.exec(block);
    if (!nm) continue;
    const text = JSON.parse(`"${nm[1]}"`);
    frames.push({ id: bounds[i].id, text });
  }
  return frames;
}

/** Measure an mp3's duration in milliseconds via ffprobe. */
async function mp3DurationMs(file) {
  const { execFile } = await import("node:child_process");
  return new Promise((resolve, reject) => {
    execFile(
      "ffprobe",
      ["-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0", file],
      (err, stdout) => {
        if (err) return reject(err);
        const sec = Number.parseFloat(stdout.trim());
        if (!Number.isFinite(sec)) {
          return reject(new Error(`ffprobe duration parse failed for ${file}: ${stdout}`));
        }
        resolve(Math.round(sec * 1000));
      },
    );
  });
}

async function synthesize(apiKey, text) {
  const res = await fetch(API_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [
        { role: "user", content: STYLE },
        { role: "assistant", content: text },
      ],
      audio: { format: "mp3", voice: VOICE },
    }),
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`MiMo API ${res.status}: ${body.slice(0, 300)}`);
  }
  const json = await res.json();
  const b64 = json?.choices?.[0]?.message?.audio?.data;
  if (!b64) {
    throw new Error(`unexpected response shape: ${JSON.stringify(json).slice(0, 300)}`);
  }
  return Buffer.from(b64, "base64");
}

async function main() {
  const apiKey = process.env.MIMO_API_KEY;
  if (!apiKey) {
    console.error("MIMO_API_KEY is not set. Extract it from the mimo section of ~/ateam/llm_api.md.");
    process.exit(1);
  }

  const source = await readFile(SCENARIO, "utf8");
  const frames = extractNarrations(source);
  console.log(`found ${frames.length} narration frames`);

  await mkdir(OUT_DIR, { recursive: true });

  const manifest = {};
  for (const { id, text } of frames) {
    const file = path.join(OUT_DIR, `${id}.mp3`);
    if (!existsSync(file)) {
      process.stdout.write(`  ${id}: synthesizing (${text.length} chars)… `);
      const mp3 = await synthesize(apiKey, text);
      await writeFile(file, mp3);
      console.log(`${(mp3.length / 1024).toFixed(0)} KB`);
    }
    manifest[id] = {
      file: `demo-narration/${id}.mp3`,
      durationMs: await mp3DurationMs(file),
    };
  }

  await writeFile(MANIFEST, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`manifest written: ${path.relative(FRONTEND, MANIFEST)} (${Object.keys(manifest).length} entries)`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
