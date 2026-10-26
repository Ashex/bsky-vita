import { afterEach, describe, expect, it } from "bun:test";
import { emitImageHandle } from "../src/native_batch";

describe("native image handle emitter", () => {
  afterEach(() => { globalThis.nativeBatch = { commands: [], image: null }; });
  it("encodes a bounded handle command in nativeBatch", () => {
    globalThis.nativeBatch = { commands: [], image: null };
    expect(emitImageHandle(17, 10, 20, 30, 40)).toBe(true);
    expect(globalThis.nativeBatch.commands).toEqual([{ type: "image_handle", handle: 17, x: 10, y: 20, width: 30, height: 40 }]);
  });
  it("rejects invalid geometry, handles, and command overflow", () => {
    globalThis.nativeBatch = { commands: [], image: null };
    for (const args of [[0, 0, 0, 1, 1], [0x1_0000_0000, 0, 0, 1, 1], [1, -1, 0, 1, 1], [1, 479, 0, 2, 1], [1, 0, 271, 1, 2], [1, 0, 0, 0, 1], [1, NaN, 0, 1, 1]] as number[][]) {
      expect(emitImageHandle(args[0]!, args[1]!, args[2]!, args[3]!, args[4]!)).toBe(false);
    }
    globalThis.nativeBatch.commands = Array.from({ length: 8 }, () => ({}));
    expect(emitImageHandle(1, 0, 0, 1, 1)).toBe(false);
    expect(globalThis.nativeBatch.commands).toHaveLength(8);
  });
});
