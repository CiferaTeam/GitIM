export interface Device {
  id: string;
  registered_at: string;
  last_seen: string;
}

export interface InviteCode {
  code: string;
  created_at: string;
  max_devices: number;
  note: string;
  devices: Device[];
}

export type Bindings = {
  CELL_GITIM_KV: KVNamespace;
  ADMIN_SECRET: string;
};

export function kvKey(code: string): string {
  return `invite:${code}`;
}
