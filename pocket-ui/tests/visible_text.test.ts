import { describe, expect, it } from "bun:test";
import {
  firstCandidates,
  layoutRows,
  moveSelection,
  reconcileSelection,
  visibleWindow,
  parseMeasureRequest,
  validatePlacement,
  validateReply,
  utf8Bytes,
  type Metrics,
} from "../src/visible_text";

describe("visible text contract", () => {
  const request = {
    schema: 1 as const,
    stamp: {
      auth_generation: 1,
      mount_generation: 2,
      history_revision: 3,
      layout_id: 4,
    },
    rows: [
      { uri: "at://one", width: 128 },
      { uri: "at://two", width: 128 },
    ],
  };
  it("rejects malformed, unknown, stale, misaligned and oversized JSON", () => {
    expect(parseMeasureRequest(JSON.stringify(request))).not.toBeNull();
    expect(
      parseMeasureRequest(JSON.stringify({ ...request, extra: 1 })),
    ).toBeNull();
    expect(parseMeasureRequest(" ".repeat(4097))).toBeNull();
    expect(
      validateReply(
        { schema: 1, stamp: { ...request.stamp, layout_id: 5 }, rows: [] },
        request,
      ),
    ).toBe(false);
    expect(
      validatePlacement(
        {
          schema: 1,
          stamp: request.stamp,
          rows: [
            { uri: "wrong", x: 0, y: 0, body_height: 20 },
            { uri: "at://two", x: 0, y: 1, body_height: 20 },
          ],
        },
        request,
      ),
    ).toBe(false);
  });
  it("selects the first three history entries without filtering hidden entries", () => {
    const h = [
      { uri: "hidden" },
      { uri: "two" },
      { uri: "three" },
      { uri: "four" },
    ];
    expect(firstCandidates(h).map((x) => x.uri)).toEqual([
      "hidden",
      "two",
      "three",
    ]);
    expect(h).toHaveLength(4);
  });
  it("keeps a selected URI anchored as history appends or replaces", () => {
    const original = Array.from({ length: 200 }, (_, i) => ({ uri: `u${i}` }));
    let selected: string | null = "u2";
    selected = moveSelection(original, selected, 1);
    for (let i = 200; i < 240; i++) original.push({ uri: `u${i}` });
    expect(selected).toBe("u3");
    expect(visibleWindow(original, selected).map((post) => post.uri)).toEqual(["u3", "u4", "u5"]);
    const replaced = original.filter((post) => post.uri !== selected);
    expect(reconcileSelection(original, replaced, selected)).toBe("u4");
    expect(reconcileSelection(original, [], selected)).toBeNull();
    expect(moveSelection(original, "u0", -1)).toBe("u0");
    expect(moveSelection(original, "u239", 1)).toBe("u239");
    expect(firstCandidates(original).map((post) => post.uri)).toEqual(["u0", "u1", "u2"]);
    expect(visibleWindow(original, "u199").map((post) => post.uri)).toEqual(["u199", "u200", "u201"]);
  });
  it("counts UTF-8 by bytes for astral and combining text", () =>
    expect(utf8Bytes("🛰e\u0301\0")).toBe(8));
  it("lays out mixed text and placeholders as non-overlapping measured cards", () => {
    const metrics: Metrics[] = [
      {
        uri: "at://one",
        status: "ready",
        width: 440,
        line_count: 1,
        line_height: 18,
        baseline_offset: 14,
        body_height: 18,
        continuation: false,
        context: "reply",
      },
      {
        uri: "at://two",
        status: "ready",
        width: 440,
        line_count: 2,
        line_height: 18,
        baseline_offset: 14,
        body_height: 36,
        continuation: false,
        context: "quote",
      },
      {
        uri: "at://hidden",
        status: "placeholder",
        width: 440,
        line_count: 0,
        line_height: 18,
        baseline_offset: 14,
        body_height: 18,
        continuation: false,
      },
    ];
    const rows = layoutRows(
      metrics.map(({ context: _context, ...row }) => row),
    );
    const contextRows = layoutRows(
      metrics.map((row, i) => ({
        ...row,
        context: row.status === "ready" && i === 0 ? ("reply" as const) : null,
      })),
    );
    expect(contextRows[0]!.body_y - rows[0]!.body_y).toBe(14);
    expect(contextRows[0]!.height - rows[0]!.height).toBe(14);
    expect(contextRows[1]!.body_y - rows[1]!.body_y).toBe(14);
    expect(contextRows[1]!.height - rows[1]!.height).toBe(0);
    expect(contextRows[2]!.body_y - rows[2]!.body_y).toBe(14);
    expect(contextRows[2]!.height - rows[2]!.height).toBe(0);
    expect(contextRows.map((r) => r.width)).toEqual([456, 456, 456]);
    expect(contextRows.map((r) => r.body_x)).toEqual([20, 20, 20]);
    expect(contextRows.map((r) => r.context_height)).toEqual([12, 0, 0]);
    expect(
      rows.map((r) => [
        r.x,
        r.y,
        r.width,
        r.height,
        r.body_x,
        r.body_y,
        r.body_height,
      ]),
    ).toEqual([
      [12, 40, 456, 76, 20, 74, 18],
      [12, 124, 456, 94, 20, 158, 36],
      [12, 226, 456, 76, 20, 260, 18],
    ]);
    expect(rows[0]!.y + rows[0]!.height).toBeLessThan(rows[1]!.y);
    expect(rows[1]!.y + rows[1]!.height).toBeLessThan(rows[2]!.y);
    expect(contextRows[0]!.y + contextRows[0]!.height).toBeLessThan(
      contextRows[1]!.y,
    );
    expect(contextRows[1]!.y + contextRows[1]!.height).toBeLessThan(
      contextRows[2]!.y,
    );
    const req = {
      ...request,
      rows: metrics.map((m) => ({ uri: m.uri, width: m.width })),
    };
    const packed = rows.map((r) => ({
      uri: r.uri,
      x: r.body_x,
      y: r.body_y,
      body_height: r.body_height,
    }));
    expect(
      validatePlacement({ schema: 1, stamp: req.stamp, rows: packed }, req),
    ).toBe(true);
    expect(
      packed.every(
        (r, i) =>
          r.x === rows[i]!.body_x &&
          r.y === rows[i]!.body_y &&
          r.body_height === rows[i]!.body_height,
      ),
    ).toBe(true);
  });
});
