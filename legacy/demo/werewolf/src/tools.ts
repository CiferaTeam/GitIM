import net from "node:net";
import readline from "node:readline";

// ── Socket Client ─────────────────────────────────────────
//
//  Line-delimited JSON over Unix socket.
//  Protocol 同 cli/src/client.ts.

export async function callDaemon(
  socketPath: string,
  payload: Record<string, unknown>
): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    const message = JSON.stringify(payload) + "\n";

    socket.on("connect", () => socket.write(message));

    const rl = readline.createInterface({ input: socket });
    rl.on("error", () => {});
    rl.on("line", (line: string) => {
      try {
        const json = JSON.parse(line);
        if (!json.ok) reject(new Error(json.error ?? "daemon error"));
        else resolve(json.data);
      } catch {
        reject(new Error(`Invalid response: ${line}`));
      }
      socket.end();
    });

    socket.on("error", (err: Error) => {
      reject(new Error(`Cannot connect to daemon: ${err.message}`));
    });
  });
}
