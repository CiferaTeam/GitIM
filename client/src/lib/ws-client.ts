import type { WsRequest, WsResponse, PushEvent } from './types.js';

type PushHandler = (event: PushEvent) => void;

/**
 * WebSocket 客户端 — 所有通信走一条 WebSocket 连接
 * 请求/响应通过 id 匹配，推送事件通过 event 字段识别
 */
export class WsClient {
  private ws: WebSocket | null = null;
  private url: string;
  private reqId = 0;
  private pending = new Map<number, {
    resolve: (res: WsResponse) => void;
    timer: ReturnType<typeof setTimeout>;
  }>();
  private pushHandlers: PushHandler[] = [];
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _connected = false;
  onConnectionChange?: (connected: boolean) => void;

  constructor(url: string) {
    this.url = url;
  }

  get connected() { return this._connected; }

  connect() {
    if (this.ws) return;
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      this._connected = true;
      this.onConnectionChange?.(true);
    };

    this.ws.onclose = () => {
      this._connected = false;
      this.ws = null;
      this.onConnectionChange?.(false);
      // 自动重连
      this.reconnectTimer = setTimeout(() => this.connect(), 2000);
    };

    this.ws.onmessage = (ev) => {
      let data: any;
      try { data = JSON.parse(ev.data); } catch { return; }

      // 推送事件
      if (data.event) {
        for (const h of this.pushHandlers) h(data as PushEvent);
        return;
      }

      // 响应
      if (data.id != null && this.pending.has(data.id)) {
        const p = this.pending.get(data.id)!;
        clearTimeout(p.timer);
        this.pending.delete(data.id);
        p.resolve(data as WsResponse);
      }
    };
  }

  disconnect() {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
    this.ws = null;
    this._connected = false;
  }

  /** 发送请求并等待响应 */
  async request(method: string, params: Record<string, unknown> = {}): Promise<WsResponse> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return { id: 0, ok: false, error: '未连接' };
    }

    const id = ++this.reqId;
    const msg: WsRequest = { id, method, ...params };

    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve({ id, ok: false, error: '请求超时' });
      }, 10000);

      this.pending.set(id, { resolve, timer });
      this.ws!.send(JSON.stringify(msg));
    });
  }

  /** 注册推送事件处理器 */
  onPush(handler: PushHandler) {
    this.pushHandlers.push(handler);
    return () => {
      this.pushHandlers = this.pushHandlers.filter(h => h !== handler);
    };
  }
}
