/**
 * Backend interface — abstracts the communication layer between webui-v2 and
 * the IM engine. Two implementations:
 *   - HttpBackend: talks to gitim-runtime via HTTP (desktop, current behavior)
 *   - LocalBackend: talks to daemon-web via Web Worker (mobile, Phase 1+)
 */
import type { ApiResponse } from "./types";

export interface Backend {
  health(): Promise<ApiResponse>;
  me(): Promise<ApiResponse>;
  poll(since?: string): Promise<ApiResponse>;
  channels(): Promise<ApiResponse>;
  read(channel: string, limit?: number): Promise<ApiResponse>;
  send(
    channel: string,
    body: string,
    author?: string,
    replyTo?: number,
  ): Promise<ApiResponse>;
  thread(channel: string, line: number): Promise<ApiResponse>;
  users(): Promise<ApiResponse>;
  joinChannel(channel: string): Promise<ApiResponse>;
}

export class HttpBackend implements Backend {
  private baseUrl: () => string;

  constructor(baseUrl: () => string) {
    this.baseUrl = baseUrl;
  }

  async health(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/health`);
    if (!res.ok)
      return { ok: false, error: `health check failed: ${res.status}` };
    const data = await res.json();
    return { ok: true, data };
  }

  async me(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/me`);
    return await res.json();
  }

  async poll(since?: string): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ since }),
    });
    return await res.json();
  }

  async channels(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/channels`);
    return await res.json();
  }

  async read(channel: string, limit?: number): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, limit }),
    });
    return await res.json();
  }

  async send(
    channel: string,
    body: string,
    _author?: string,
    replyTo?: number,
  ): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, body, reply_to: replyTo }),
    });
    return await res.json();
  }

  async thread(channel: string, line: number): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/thread`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, line }),
    });
    return await res.json();
  }

  async users(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/users`);
    return await res.json();
  }

  async joinChannel(channel: string): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/join`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel }),
    });
    return await res.json();
  }
}
