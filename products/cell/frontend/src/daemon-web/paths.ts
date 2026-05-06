export type ThreadTarget =
  | {
      kind: "channel";
      name: string;
      threadPath: string;
    }
  | {
      kind: "dm";
      name: string;
      threadPath: string;
      members: [string, string];
    };

const HANDLER_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
const CHANNEL_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export function validateHandler(handler: string): string | null {
  if (!handler) return "handler is empty";
  if (handler.length > 39) return "handler exceeds 39 characters";
  if (handler === "system") return "handler 'system' is reserved";
  if (!HANDLER_RE.test(handler) || handler.includes("--")) {
    return `invalid handler: ${handler}`;
  }
  return null;
}

export function validateChannelName(channel: string): string | null {
  if (!channel) return "channel name is empty";
  if (channel.length > 32) return "channel name exceeds 32 characters";
  if (!CHANNEL_RE.test(channel)) return `invalid channel name: ${channel}`;
  return null;
}

export function channelMetaPath(channel: string): string {
  const error = validateChannelName(channel);
  if (error) throw new Error(error);
  return `channels/${channel}.meta.yaml`;
}

export function resolveThreadTarget(channel: string): ThreadTarget {
  if (channel.startsWith("dm:")) {
    const members = channel.slice(3).split(",");
    if (members.length !== 2) {
      throw new Error("DM format must be dm:handler1,handler2");
    }
    return resolveDmMembers(members[0], members[1]);
  }

  if (channel.includes("--")) {
    const members = channel.split("--");
    if (members.length !== 2) {
      throw new Error(`invalid DM name: ${channel}`);
    }
    return resolveDmMembers(members[0], members[1]);
  }

  const error = validateChannelName(channel);
  if (error) throw new Error(error);
  return {
    kind: "channel",
    name: channel,
    threadPath: `channels/${channel}.thread`,
  };
}

export function channelNameFromMetaFile(fileName: string): string | null {
  if (!fileName.endsWith(".meta.yaml")) return null;
  const name = fileName.slice(0, -".meta.yaml".length);
  return validateChannelName(name) ? null : name;
}

export function channelNameFromThreadPath(path: string): string | null {
  if (!path.startsWith("channels/") || !path.endsWith(".thread")) return null;
  const name = path.slice("channels/".length, -".thread".length);
  return validateChannelName(name) ? null : name;
}

export function dmApiNameFromThreadPath(path: string): string | null {
  if (!path.startsWith("dm/") || !path.endsWith(".thread")) return null;
  const name = path.slice("dm/".length, -".thread".length);
  try {
    const target = resolveThreadTarget(name);
    if (target.kind !== "dm") return null;
    return `dm:${target.members[0]},${target.members[1]}`;
  } catch {
    return null;
  }
}

function resolveDmMembers(first: string, second: string): ThreadTarget {
  const firstError = validateHandler(first);
  if (firstError) throw new Error(firstError);
  const secondError = validateHandler(second);
  if (secondError) throw new Error(secondError);

  const sorted = [first, second].sort() as [string, string];
  const name = `${sorted[0]}--${sorted[1]}`;
  return {
    kind: "dm",
    name,
    members: sorted,
    threadPath: `dm/${name}.thread`,
  };
}
