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
}

/** 用户信息 */
export interface UserInfo {
  handler: string;
  display_name: string;
}

/** WebSocket 请求 */
export interface WsRequest {
  id: number;
  method: string;
  [key: string]: unknown;
}

/** WebSocket 响应 */
export interface WsResponse {
  id: number;
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
}

/** 推送事件 */
export interface PushEvent {
  event: string;
  channel: string;
  kind: string;
}

/** 格式化时间戳 20260317T120000Z → 12:00 */
export function formatTimestamp(ts: string): string {
  const match = ts.match(/T(\d{2})(\d{2})\d{2}Z$/);
  if (!match) return '??:??';
  return `${match[1]}:${match[2]}`;
}
