import { useSyncExternalStore } from "react";

function subscribe(callback: () => void, query: string) {
  const mql = window.matchMedia(query);
  mql.addEventListener("change", callback);
  return () => mql.removeEventListener("change", callback);
}

function getSnapshot(query: string) {
  return window.matchMedia(query).matches;
}

function getServerSnapshot() {
  return false;
}

export function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    (cb) => subscribe(cb, query),
    () => getSnapshot(query),
    () => getServerSnapshot()
  );
}

export function useIsMobile(): boolean {
  return useMediaQuery("(max-width: 768px)");
}
