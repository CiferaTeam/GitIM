import net from 'node:net';
import path from 'node:path';
import readline from 'node:readline';

export interface ApiResponse {
  ok: boolean;
  data?: any;
  error?: string;
}

export class GitimClient {
  private socketPath: string;

  constructor(repoRoot: string) {
    this.socketPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');
  }

  async request(method: string, params: Record<string, any> = {}): Promise<ApiResponse> {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection(this.socketPath);
      const payload = JSON.stringify({ method, ...params }) + '\n';

      socket.on('connect', () => {
        socket.write(payload);
      });

      const rl = readline.createInterface({ input: socket });
      rl.on('line', (line: string) => {
        try {
          resolve(JSON.parse(line));
        } catch {
          reject(new Error(`Invalid response: ${line}`));
        }
        socket.end();
      });

      socket.on('error', (err: Error) => {
        reject(new Error(`Cannot connect to daemon: ${err.message}`));
      });
    });
  }

  async status(): Promise<ApiResponse> {
    return this.request('status');
  }

  async send(channel: string, body: string, author?: string, replyTo?: number): Promise<ApiResponse> {
    return this.request('send', { channel, body, author: author ?? null, reply_to: replyTo ?? null });
  }

  async read(channel: string, limit?: number, since?: number): Promise<ApiResponse> {
    return this.request('read', { channel, limit: limit ?? null, since: since ?? null });
  }

  async listChannels(): Promise<ApiResponse> {
    return this.request('channels');
  }

  async listUsers(): Promise<ApiResponse> {
    return this.request('users');
  }

  async getThread(channel: string, lineNumber: number): Promise<ApiResponse> {
    return this.request('thread', { channel, line_number: lineNumber });
  }

  async registerUser(handler: string, displayName: string, role?: string, introduction?: string): Promise<ApiResponse> {
    return this.request('register_user', {
      handler,
      display_name: displayName,
      role: role ?? 'member',
      introduction: introduction ?? 'GitIM user',
    });
  }

  async onboard(gitServer: string, auth: Record<string, string>): Promise<ApiResponse> {
    return this.request('onboard', { git_server: gitServer, auth });
  }

  async stop(): Promise<ApiResponse> {
    return this.request('stop');
  }

  async poll(since?: string): Promise<ApiResponse> {
    return this.request('poll', { since: since ?? null });
  }

  async search(params: {
    query?: string;
    author?: string;
    channel?: string;
    channel_type?: string;
    limit?: number;
    offset?: number;
  }): Promise<ApiResponse> {
    return this.request('search', {
      query: params.query ?? null,
      author: params.author ?? null,
      channel: params.channel ?? null,
      channel_type: params.channel_type ?? null,
      limit: params.limit ?? 50,
      offset: params.offset ?? 0,
    });
  }

  async reindex(): Promise<ApiResponse> {
    return this.request('reindex');
  }
}
