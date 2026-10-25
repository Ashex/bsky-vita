import { describe, expect, it } from "bun:test";
import { parseDelivery, requestFeed, takeFeedCommands, utf8ByteLength } from "../src/feed";

describe("guest feed boundary", () => {
  const good = { schema: 1, correlation: { schema: 1, auth_generation: 1, mount_generation: 2, request_id: 3, source: "following", cursor: null }, posts: [{ uri: "at://p", cid: "c", author: { did: "did:x", handle: "x.test", display_name: "X" }, text: "hello", facets: [], like_count: 0, repost_count: 0, reply_count: 0, state: { state: "clean" }, quote: null, images: [], context: "reply" }], cursor: null };
  it("accepts bounded safe delivery and rejects malformed/oversized delivery", () => {
    expect(parseDelivery(JSON.stringify(good))?.posts[0]?.text).toBe("hello");
    expect(parseDelivery(JSON.stringify({ ...good, posts: [{ ...good.posts[0], context: "arbitrary private text" }] }))).toBeNull();
    expect(parseDelivery(JSON.stringify({ ...good, correlation: { ...good.correlation, request_id: 0 } }))).toBeNull();
    expect(parseDelivery(JSON.stringify({ ...good, posts: Array(201).fill(good.posts[0]) }))).toBeNull();
    expect(parseDelivery("{" )).toBeNull();
  });
  it("rejects hidden context markers and content before delivery serialization", () => {
    const hidden = { ...good, posts: [{ ...good.posts[0], text: "SECRET_BODY", context: "quote" }] };
    hidden.posts[0]!.state = { state: "blocked" };
    // Malformed hidden metadata is rejected; the live runtime fixture uses
    // context: null so valid blocked rows do not invalidate the full delivery.
    expect(parseDelivery(JSON.stringify(hidden))).toBeNull();
    const validHidden = {
      ...good,
      posts: [{ ...good.posts[0]!, state: { state: "blocked" }, context: null, text: "", images: [], quote: null }],
    };
    expect(parseDelivery(JSON.stringify(validHidden))).toBeNull();
    hidden.posts[0]!.text = "";
    delete hidden.posts[0]!.context;
    expect(JSON.stringify(hidden)).not.toContain("SECRET_BODY");
    expect(JSON.stringify(hidden)).not.toContain("context");
    expect(parseDelivery(JSON.stringify(hidden))?.posts[0]?.state.state).toBe("blocked");
  });
  it("validates tagged state and measures UTF-8 without TextEncoder", () => {
    expect(utf8ByteLength("A🛰é\ud800")).toBe(10);
    const withoutEncoder = globalThis.TextEncoder;
    // The Vita guest has no TextEncoder global.
    Object.defineProperty(globalThis, "TextEncoder", { configurable: true, value: undefined });
    try {
      expect(parseDelivery(JSON.stringify(good))?.posts).toHaveLength(1);
      expect(parseDelivery(JSON.stringify({ ...good, posts: [{ ...good.posts[0], state: null }] }))).toBeNull();
      expect(parseDelivery(JSON.stringify({ ...good, posts: [{ ...good.posts[0], state: "clean" }] }))).toBeNull();
      const emojiTooLarge = { ...good, posts: [{ ...good.posts[0], text: "🛰".repeat(16_385) }] };
      expect(parseDelivery(JSON.stringify(emojiTooLarge))).toBeNull();
    } finally {
      Object.defineProperty(globalThis, "TextEncoder", { configurable: true, value: withoutEncoder });
    }
  });
  it("admits only feed actions", () => {
    const calls: string[] = [];
    globalThis.__bskyRequestFeed = command => calls.push(command);
    expect(requestFeed("refresh")).toBe(true);
    expect(requestFeed("eval" )).toBe(false);
    expect(takeFeedCommands()).toEqual(["refresh"]);
    expect(calls).toEqual([]);
    expect(Array.from({ length: 128 }, () => requestFeed("next_page")).every(Boolean)).toBe(true);
    expect(requestFeed("leave")).toBe(false);
    expect(takeFeedCommands()).toHaveLength(128);
  });
});
