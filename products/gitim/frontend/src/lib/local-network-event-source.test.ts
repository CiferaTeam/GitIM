import { describe, expect, it, vi } from "vitest";
import {
  createLocalNetworkEventSource,
  parseSseEventBlock,
} from "./local-network-event-source";

describe("parseSseEventBlock", () => {
  it("joins data lines from one SSE event", () => {
    expect(parseSseEventBlock("event: message\ndata: one\ndata: two")).toBe(
      "one\ntwo",
    );
  });

  it("ignores blocks without data lines", () => {
    expect(parseSseEventBlock(": keepalive")).toBeNull();
  });
});

describe("createLocalNetworkEventSource", () => {
  it("fetches loopback event streams with loopback target metadata", async () => {
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode("data: hello\n\n"));
        controller.close();
      },
    });
    const fetchMock = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(new Response(stream));
    const messages: string[] = [];

    const source = createLocalNetworkEventSource(
      "http://127.0.0.1:16868/fleet/events",
    );
    source.onmessage = (event) => messages.push(event.data);

    await vi.waitFor(() => {
      expect(messages).toEqual(["hello"]);
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://127.0.0.1:16868/fleet/events",
      expect.objectContaining({
        headers: { Accept: "text/event-stream" },
        targetAddressSpace: "loopback",
      }),
    );

    source.close();
  });
});
