import { describe, expect, it, vi } from "vitest";
import { localNetworkFetch } from "./local-network-fetch";

describe("localNetworkFetch", () => {
  it("marks loopback runtime requests as loopback targets", async () => {
    const response = new Response("ok");
    const fetchMock = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(response);
    const signal = new AbortController().signal;

    const result = await localNetworkFetch("http://127.0.0.1:16868/health", {
      cache: "no-store",
      signal,
    });

    expect(result).toBe(response);
    expect(fetchMock).toHaveBeenCalledWith(
      "http://127.0.0.1:16868/health",
      expect.objectContaining({
        cache: "no-store",
        signal,
        targetAddressSpace: "loopback",
      }),
    );
  });

  it("marks private address requests as local network targets", async () => {
    const response = new Response("ok");
    const fetchMock = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(response);

    await localNetworkFetch("http://192.168.1.10/health");

    expect(fetchMock).toHaveBeenCalledWith(
      "http://192.168.1.10/health",
      expect.objectContaining({
        targetAddressSpace: "local",
      }),
    );
  });
});
