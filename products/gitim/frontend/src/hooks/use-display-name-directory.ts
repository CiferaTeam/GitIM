import { createContext, useContext } from "react";
import type { Agent, UserInfo } from "../lib/types";

/**
 * The display-name directory: a `handler → display_name` map the chat UI reads
 * at render time. Empty map = the default context value, so consumers outside
 * the provider (e.g. isolated component tests) degrade to bare handlers rather
 * than crash.
 */
export const DirectoryContext = createContext<ReadonlyMap<string, string>>(
  new Map(),
);

export function useDirectory(): ReadonlyMap<string, string> {
  return useContext(DirectoryContext);
}

/**
 * Build the directory from the two sources that actually carry display_name:
 * agents (`/agents`) and registered humans (`list_users`' `user_infos`). The
 * current user needs no separate source — they're a registered user and so
 * appear in `user_infos`; `/im/me` only supplies the current user's *handler*.
 *
 * A `display_name === handler` entry adds nothing (resolveDisplayName discards
 * it anyway), so it's skipped to keep the map lean.
 */
export function buildDirectory(
  agents: readonly Agent[],
  userInfos: readonly UserInfo[],
): Map<string, string> {
  const map = new Map<string, string>();
  for (const u of userInfos) {
    if (u.display_name && u.display_name !== u.handler) {
      map.set(u.handler, u.display_name);
    }
  }
  // `agent.name` is `display_name ?? handler`; only a real display_name
  // (≠ handler) belongs in the directory.
  for (const a of agents) {
    const h = a.handler ?? a.id;
    if (a.name && a.name !== h) map.set(h, a.name);
  }
  return map;
}
