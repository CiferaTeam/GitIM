import type { ApiResponse } from './types.js';

const BASE = '';  // same origin — relative URLs

async function get(path: string, params?: Record<string, string>): Promise<ApiResponse> {
  try {
    let url = `${BASE}${path}`;
    if (params) {
      const qs = new URLSearchParams(params).toString();
      if (qs) url += `?${qs}`;
    }
    const res = await fetch(url);
    return await res.json() as ApiResponse;
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

async function post(path: string, body: Record<string, unknown>): Promise<ApiResponse> {
  try {
    const res = await fetch(`${BASE}${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    return await res.json() as ApiResponse;
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

// ---------- API methods ----------

export function me(): Promise<ApiResponse> {
  return get('/api/me');
}

export function poll(since?: string): Promise<ApiResponse> {
  const params: Record<string, string> = {};
  if (since) params.since = since;
  return get('/api/poll', params);
}

export function channels(): Promise<ApiResponse> {
  return get('/api/channels');
}

export function users(): Promise<ApiResponse> {
  return get('/api/users');
}

export function read(channel: string, limit?: number): Promise<ApiResponse> {
  const params: Record<string, string> = { channel };
  if (limit != null) params.limit = String(limit);
  return get('/api/read', params);
}

export function thread(channel: string, line: number): Promise<ApiResponse> {
  return get('/api/thread', { channel, line: String(line) });
}

export function send(
  channel: string,
  body: string,
  author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const payload: Record<string, unknown> = { channel, body };
  if (author) payload.author = author;
  if (replyTo != null && replyTo > 0) payload.reply_to = replyTo;
  return post('/api/send', payload);
}
