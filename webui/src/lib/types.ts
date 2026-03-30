/** 消息发送状态 */
export type MessageStatus = 'sending' | 'sent' | 'synced' | 'failed';

/** GitIM 消息 */
export interface Message {
  line_number: number;
  point_to: number;  // 0=根消息, >0=回复目标行号
  author: string;
  timestamp: string; // 20260317T120000Z
  body: string;
  /** 前端本地状态（daemon 返回的消息无此字段） */
  _status?: MessageStatus;
  /** 前端临时 ID（pending 消息用） */
  _pendingId?: string;
}

/** 频道信息 */
export interface Channel {
  name: string;
  kind: 'channel' | 'dm';
  unreadCount: number;
  members: string[];
}

/** 用户信息 */
export interface UserInfo {
  handler: string;
  display_name: string;
}

/** HTTP API 响应 */
export interface ApiResponse {
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
}

/** 轮询变更项 */
export interface PollChange {
  channel: string;
  kind: string;
}

/** 格式化时间戳 20260317T120000Z → 12:00 */
export function formatTimestamp(ts: string): string {
  const match = ts.match(/T(\d{2})(\d{2})\d{2}Z$/);
  if (!match) return '??:??';
  return `${match[1]}:${match[2]}`;
}
