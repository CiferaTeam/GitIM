import { localNetworkFetch } from "./local-network-fetch";

export interface LocalNetworkEventSource {
  onmessage: ((event: MessageEvent<string>) => void) | null;
  onerror: ((event: Event) => void) | null;
  close: () => void;
}

export function parseSseEventBlock(block: string): string | null {
  const data: string[] = [];
  for (const line of block.split(/\r?\n/)) {
    if (!line.startsWith("data:")) continue;
    data.push(line.slice(5).replace(/^ /, ""));
  }
  return data.length > 0 ? data.join("\n") : null;
}

class FetchEventSource implements LocalNetworkEventSource {
  onmessage: ((event: MessageEvent<string>) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  private readonly controller = new AbortController();
  private closed = false;
  private readonly url: string;

  constructor(url: string) {
    this.url = url;
    void this.connect();
  }

  close() {
    this.closed = true;
    this.controller.abort();
  }

  private async connect() {
    try {
      const res = await localNetworkFetch(this.url, {
        headers: { Accept: "text/event-stream" },
        signal: this.controller.signal,
      });
      if (!res.ok || !res.body) throw new Error(`SSE failed: ${res.status}`);

      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        buffer = this.drain(buffer);
      }

      buffer += decoder.decode();
      this.drain(`${buffer}\n\n`);
    } catch {
      if (!this.closed) this.onerror?.(new Event("error"));
    }
  }

  private drain(buffer: string): string {
    const parts = buffer.split(/\r?\n\r?\n/);
    const rest = parts.pop() ?? "";
    for (const part of parts) {
      const data = parseSseEventBlock(part);
      if (data !== null) {
        this.onmessage?.(new MessageEvent("message", { data }));
      }
    }
    return rest;
  }
}

export function createLocalNetworkEventSource(url: string): LocalNetworkEventSource {
  return new FetchEventSource(url);
}
