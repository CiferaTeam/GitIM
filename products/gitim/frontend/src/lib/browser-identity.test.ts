import { describe, expect, it, vi } from "vitest";
import { inferBrowserIdentity } from "./browser-identity";

vi.mock("gitim-wasm", () => ({
  default: vi.fn(async () => ({})),
  githubIdentityFromUserJson: vi.fn((body: string) => {
    const data = JSON.parse(body) as { login: string; name?: string | null; email?: string | null };
    return {
      handler: data.login.toLowerCase(),
      display_name: data.name ?? data.login.toLowerCase(),
      email: data.email ?? null,
    };
  }),
}));

describe("browser identity inference", () => {
  it("fetches GitHub user JSON and parses it through gitim-wasm", async () => {
    const fetcher = vi.fn(async () =>
      new Response(JSON.stringify({ login: "Flame4", name: "Flame" }), {
        status: 200,
      }),
    );

    const identity = await inferBrowserIdentity({
      remoteUrl: "https://github.com/flame4/room",
      token: "github_pat_secret",
      fetcher,
    });

    expect(identity).toEqual({
      handler: "flame4",
      displayName: "Flame",
      email: null,
    });
    expect(fetcher).toHaveBeenCalledWith(
      "https://api.github.com/user",
      expect.objectContaining({
        headers: expect.objectContaining({
          Authorization: "token github_pat_secret",
        }),
      }),
    );
  });

  it("rejects non-GitHub remotes before fetching", async () => {
    const fetcher = vi.fn();
    await expect(
      inferBrowserIdentity({
        remoteUrl: "https://gitlab.com/team/room",
        token: "token",
        fetcher,
      }),
    ).rejects.toThrow("github.com");
    expect(fetcher).not.toHaveBeenCalled();
  });
});
