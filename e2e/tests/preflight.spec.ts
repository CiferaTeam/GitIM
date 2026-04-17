import { test, expect } from "@playwright/test";
import { buildRuntime, startEnv, stopEnv, type RuntimeEnv } from "../helpers/runtime-env";

// Route contract tests for GET /preflight/{provider}.
//
// Deliberately unguarded: the route returns 200 even when the provider CLI
// isn't installed locally (body reports `available: false, error_kind:
// "not_installed"`). We assert shape + provider identity, not success —
// success depends on whether the developer has claude/codex on PATH and
// logged in, which is not a precondition of this test.
test.describe("preflight route contract", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("unknown provider returns 400 with stable error shape", async () => {
    const res = await fetch(`${env.baseUrl}/preflight/unknown`);
    expect(res.status).toBe(400);

    const body = await res.json();
    expect(body.ok).toBe(false);
    expect(body.error).toBe("unknown provider");
  });

  test("claude returns 200 with PreflightResult shape", async () => {
    const res = await fetch(`${env.baseUrl}/preflight/claude`);
    expect(res.status).toBe(200);

    const body = await res.json();
    expect(body.provider).toBe("claude");
    expect(typeof body.available).toBe("boolean");
    expect(typeof body.duration_ms).toBe("number");
    // `error_kind` must be present (not elided) so the WebUI can branch
    // without checking for key existence — null on success, snake_case
    // string on failure.
    expect(body).toHaveProperty("error_kind");
  });

  test("codex returns 200 with PreflightResult shape", async () => {
    const res = await fetch(`${env.baseUrl}/preflight/codex`);
    expect(res.status).toBe(200);

    const body = await res.json();
    expect(body.provider).toBe("codex");
    expect(typeof body.available).toBe("boolean");
    expect(typeof body.duration_ms).toBe("number");
    expect(body).toHaveProperty("error_kind");
  });
});
