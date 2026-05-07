import type { AuthCallback } from "isomorphic-git";

export function tokenAuth(token: string): AuthCallback {
  return () => ({ username: token });
}
