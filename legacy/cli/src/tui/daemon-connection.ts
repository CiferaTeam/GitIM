/**
 * DaemonConnection — 持久连接 + 订阅推送
 *
 * 保持一个到 daemon 的长连接，支持：
 * 1. request-response 模式（发送请求，等待响应）
 * 2. subscribe 模式（接收实时推送事件）
 */
import net from 'node:net';
import path from 'node:path';
import readline from 'node:readline';
import { EventEmitter } from 'node:events';

export interface ApiResponse {
  ok: boolean;
  data?: any;
  error?: string;
}

export interface PushEvent {
  event: string;
  channel: string;
  kind: string;
}

export interface Message {
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;
  body: string;
}

export class DaemonConnection extends EventEmitter {
  private socketPath: string;
  private socket: net.Socket | null = null;
  private rl: readline.Interface | null = null;
  private connected = false;
  private subscribed = false;
  private pendingRequests: Map<number, {
    resolve: (res: ApiResponse) => void;
    reject: (err: Error) => void;
  }> = new Map();
  private requestId = 0;

  /** Callback for push events (set by consumer) */
  onEvent: ((event: PushEvent) => void) | null = null;

  constructor(repoRoot: string) {
    super();
    this.socketPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');
  }

  get isConnected(): boolean {
    return this.connected;
  }

  /** Connect to daemon and optionally subscribe for push events */
  async connect(subscribe = true): Promise<void> {
    return new Promise((resolve, reject) => {
      this.socket = net.createConnection(this.socketPath);

      this.socket.on('connect', async () => {
        this.connected = true;
        this.rl = readline.createInterface({ input: this.socket! });

        this.rl.on('line', (line: string) => {
          this.handleLine(line);
        });

        if (subscribe) {
          try {
            await this.request('subscribe');
            this.subscribed = true;
          } catch {
            // subscribe may not be available, continue without it
          }
        }

        this.emit('connected');
        resolve();
      });

      this.socket.on('error', (err: Error) => {
        this.connected = false;
        this.emit('error', err);
        reject(new Error(`Cannot connect to daemon: ${err.message}`));
      });

      this.socket.on('close', () => {
        this.connected = false;
        this.subscribed = false;
        this.emit('disconnected');
      });
    });
  }

  /** Send a request and wait for response */
  async request(method: string, params: Record<string, any> = {}): Promise<ApiResponse> {
    if (!this.socket || !this.connected) {
      throw new Error('Not connected to daemon');
    }

    return new Promise((resolve, reject) => {
      const id = ++this.requestId;
      const payload = JSON.stringify({ method, _req_id: id, ...params }) + '\n';

      this.pendingRequests.set(id, { resolve, reject });
      this.socket!.write(payload);

      // Timeout after 10 seconds
      setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error(`Request timeout: ${method}`));
        }
      }, 10000);
    });
  }

  /** Disconnect from daemon */
  disconnect(): void {
    if (this.rl) {
      this.rl.close();
      this.rl = null;
    }
    if (this.socket) {
      this.socket.end();
      this.socket = null;
    }
    this.connected = false;
    this.subscribed = false;
    this.pendingRequests.clear();
  }

  private handleLine(line: string): void {
    let parsed: any;
    try {
      parsed = JSON.parse(line);
    } catch {
      return;
    }

    // Check if this is a push event (has "event" field)
    if (parsed.event) {
      this.onEvent?.(parsed as PushEvent);
      this.emit('push', parsed);
      return;
    }

    // Otherwise it's a response — resolve the oldest pending request
    // (daemon doesn't echo _req_id back, so we use FIFO order)
    if (this.pendingRequests.size > 0) {
      const [id, handler] = this.pendingRequests.entries().next().value!;
      this.pendingRequests.delete(id);
      handler.resolve(parsed as ApiResponse);
    }
  }
}
