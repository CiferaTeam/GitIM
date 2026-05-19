import type { UsageBucket } from "./types";

export function usageBucketTokenTotal(
  bucket: UsageBucket,
  provider?: string,
): number {
  if (provider === "codex") {
    return bucket.input + bucket.output;
  }
  return bucket.input + bucket.output + bucket.cacheRead + bucket.cacheCreation;
}
