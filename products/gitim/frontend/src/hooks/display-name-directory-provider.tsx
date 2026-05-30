import { useMemo, type ReactNode } from "react";
import { useAgentStore } from "./use-agent-store";
import { useChatStore } from "./use-chat-store";
import { DirectoryContext, buildDirectory } from "./use-display-name-directory";

/**
 * Provides the display-name directory to the chat surface. Rebuilds the
 * `handler → display_name` map whenever the agent roster or the user roster
 * changes identity. `userInfos` is diffed at the poll-loop source so it only
 * gets a fresh identity when its content actually changes, keeping the context
 * value (and therefore every <HandlerName>) stable between polls.
 */
export function DisplayNameDirectoryProvider({
  children,
}: {
  children: ReactNode;
}) {
  const agents = useAgentStore((s) => s.agents);
  const userInfos = useChatStore((s) => s.userInfos);

  const directory = useMemo(
    () => buildDirectory(agents, userInfos),
    [agents, userInfos],
  );

  return (
    <DirectoryContext.Provider value={directory}>
      {children}
    </DirectoryContext.Provider>
  );
}
