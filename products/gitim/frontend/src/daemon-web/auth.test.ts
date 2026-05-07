import { describe, expect, it } from "vitest";
import { tokenAuth } from "./auth";

describe("daemon-web git auth", () => {
  it("passes the GitHub token as the Basic auth username", async () => {
    const auth = await tokenAuth("github_pat_secret")("https://github.com/org/repo", {});

    expect(auth).toEqual({ username: "github_pat_secret" });
  });
});
