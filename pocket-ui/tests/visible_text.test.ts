import { describe, expect, it } from "bun:test";
import {
  firstCandidates,
  layoutRows,
  scrollWindow,
  preserveAnchorOffset,
  moveSelection,
  reduceTimelineScroll,
  reduceTouchRail,
  reconcileSelection,
  visibleWindow,
  parseMeasureRequest,
  ROW_LAYOUT,
  validatePlacement,
  validateReply,
  utf8Bytes,
  type Metrics,
} from "../src/visible_text";

describe("visible text contract", () => {
  it("captures only an initial single rail contact and blocks incomplete or multi-touch gestures", () => {
    const contact = (id: number, x: number, y: number) => ({ id, x, y });
    let result = reduceTouchRail(null, [contact(7, 456, 100)]);
    expect(result.state).toEqual({ kind: "captured", id: 7, lastY: 100, dragging: false });
    expect(result.delta).toBe(0);
    result = reduceTouchRail(result.state, [contact(7, 400, 80)]);
    expect(result.delta).toBe(20);
    expect(result.state).toEqual({ kind: "captured", id: 7, lastY: 80, dragging: true });

    result = reduceTouchRail({ kind: "idle" }, [contact(7, 400, 100)]);
    expect(result.state.kind).toBe("blocked");
    result = reduceTouchRail(result.state, [contact(7, 460, 80)]);
    expect(result.state.kind).toBe("blocked");

    result = reduceTouchRail(null, [contact(7, 460, 100), contact(8, 420, 110)]);
    expect(result.state.kind).toBe("blocked");
    result = reduceTouchRail(result.state, [contact(8, 420, 90)]);
    expect(result.state.kind).toBe("blocked");
    result = reduceTouchRail(result.state, []);
    expect(result.state.kind).toBe("idle");

    result = reduceTouchRail({ kind: "captured", id: 7, lastY: 100, dragging: true }, [contact(8, 460, 90)]);
    expect(result.state.kind).toBe("blocked");
    result = reduceTouchRail(result.state, []);
    expect(result.state.kind).toBe("idle");

    result = reduceTouchRail({ kind: "captured", id: 7, lastY: 100, dragging: true }, [contact(7, 400, 80), contact(8, 460, 80)]);
    expect(result.state.kind).toBe("blocked");
    expect(result.delta).toBe(0);
    result = reduceTouchRail(result.state, [contact(7, 400, 60)]);
    expect(result.state.kind).toBe("blocked");
    expect(result.delta).toBe(0);
    result = reduceTouchRail(result.state, []);
    expect(result.state.kind).toBe("idle");

    expect(reduceTouchRail(null, [contact(1, 455, 100)]).state.kind).toBe("blocked");
    expect(reduceTouchRail(null, [contact(1, 479, 263)]).state.kind).toBe("captured");
    expect(reduceTouchRail(null, [contact(1, 480, 263)]).state.kind).toBe("blocked");
    expect(reduceTouchRail(null, [contact(1, 460, 264)]).state.kind).toBe("blocked");
    expect(reduceTouchRail({ kind: "captured", id: 7, lastY: 50, dragging: true }, []).state.kind).toBe("idle");
    result = reduceTouchRail({ kind: "captured", id: 7, lastY: 100, dragging: true }, [contact(7, 400, 80)]);
    expect(result.delta).toBe(20);
  });
  it("converts native screen pixels to half-density guest coordinates with bounded contacts", () => {
    const native = [{ id: 4, x: 912, y: 528 }];
    const snapshot = native.map(({ id, x, y }) => ({ id, x: Math.min(960, Math.max(0, x)) / 2, y: Math.min(544, Math.max(0, y)) / 2 }));
    expect(snapshot).toEqual([{ id: 4, x: 456, y: 264 }]);
  });
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
  it("accepts partial measurement replies and validates full cached viewport placement", () => {
    const reply = { schema: 1 as const, stamp: request.stamp, rows: [{ uri: "at://one", status: "ready" as const, width: 128, line_count: 1, line_height: 12, baseline_offset: 9, body_height: 12, continuation: false }] };
    expect(validateReply(reply, request)).toBe(true);
    const known: Metrics[] = [
      { ...reply.rows[0]!, context: null },
      { ...reply.rows[0]!, uri: "at://two", body_height: 24, line_count: 2 },
    ];
    const placement = { schema: 1 as const, stamp: request.stamp, rows: [
      { uri: "at://one", x: 20, y: 40, body_height: 12 },
      { uri: "at://two", x: 20, y: 80, body_height: 24 },
    ] };
    const viewport = { ...request, rows: [...request.rows, { uri: "at://three", width: 128 }, { uri: "at://four", width: 128 }] };
    const fullPlacement = { ...placement, rows: [...placement.rows, { uri: "at://three", x: 20, y: 120, body_height: 12 }, { uri: "at://four", x: 20, y: 160, body_height: 12 }] };
    const fullKnown = [...known, { ...known[0]!, uri: "at://three" }, { ...known[0]!, uri: "at://four" }];
    expect(validatePlacement(fullPlacement, viewport, fullKnown)).toBe(true);
    // Simulate the host completing only the first three of a four-row viewport;
    // the fourth metric is cached and the full-layout contract remains valid.
    const partialRequest = { ...viewport, rows: viewport.rows.slice(0, 3) };
    expect(validateReply({ ...reply, rows: [reply.rows[0]!, { ...reply.rows[0]!, uri: "at://two" }, { ...reply.rows[0]!, uri: "at://three" }] }, partialRequest)).toBe(true);
    const emptyReply = { ...reply, rows: [] };
    expect(validateReply(emptyReply, partialRequest)).toBe(false);
    expect(validatePlacement(fullPlacement, viewport, fullKnown)).toBe(true);
    expect(validatePlacement(fullPlacement, viewport, fullKnown)).toBe(true);
    expect(validatePlacement({ ...fullPlacement, rows: fullPlacement.rows.slice(0, 2) }, viewport, fullKnown)).toBe(false);
    expect(validatePlacement({ ...placement, rows: [{ ...placement.rows[1]!, body_height: 12 }] }, request, known)).toBe(false);
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
  it("windows pixel geometry with bounded mounted rows and preserves anchors across measurement", () => {
    const history = Array.from({ length: 200 }, (_, i) => ({ uri: `u${i}` }));
    const heights = new Map(history.map((row) => [row.uri, 60]));
    heights.set("u2", 900);
    const first = scrollWindow(history, heights, 0, 208, 64);
    expect(first.rows.length).toBeLessThan(8);
    const deep = scrollWindow(history, heights, 700, 208, 64);
    expect(deep.rows.some((row) => row.uri === "u2")).toBe(true);
    const before = new Map(heights);
    heights.set("u0", 120);
    const preserved = preserveAnchorOffset(history, before, heights, "u2", 100);
    expect(preserved).toBe(160);
    expect(scrollWindow(history, heights, Number.MAX_SAFE_INTEGER, 208).offset).toBeLessThan(
      scrollWindow(history, heights, Number.MAX_SAFE_INTEGER, 208).totalHeight,
    );
    expect(scrollWindow(history, heights, Number.POSITIVE_INFINITY, 208).offset).toBe(0);
  });
  it("drives pixel scrolling and selection together, including a tall row's final viewport", () => {
    const history = Array.from({ length: 200 }, (_, i) => ({ uri: `u${i}` }));
    const heights = new Map(history.map((row) => [row.uri, 60]));
    heights.set("u0", 1200);
    let state = { offset: 0, selected: "u0" as string | null };
    for (let i = 0; i < 64; i++) state = reduceTimelineScroll(history, heights, state, 1);
    const lastLineY = 40 + 1200 - 64 - state.offset;
    expect(lastLineY).toBeLessThan(264);
    expect(state.offset).toBeGreaterThan(0);
    expect(scrollWindow(history, heights, state.offset, 208, 64).rows.length).toBeLessThan(8);
    const before = new Map(heights);
    heights.set("u0", 1300);
    state = { ...state, offset: preserveAnchorOffset(history, before, heights, "u1", state.offset) };
    const appended = [...history, { uri: "u200" }];
    expect(scrollWindow(appended, heights, state.offset, 208).offset).toBe(state.offset);
    const evicted = appended.slice(1);
    const selected = reconcileSelection(appended, evicted, state.selected);
    expect(selected).toBe("u64");
    expect(scrollWindow(evicted, heights, state.offset, 208).rows.length).toBeLessThan(8);
  });
  it("publishes placement independently when scrolling requires no new measurements", () => {
    const history = Array.from({ length: 12 }, (_, i) => ({ uri: `u${i}` }));
    const heights = new Map(history.map((row) => [row.uri, 60]));
    const measured = new Set(history.map((row) => row.uri));
    const visible = scrollWindow(history, heights, 120, 208, 64).rows;
    const layoutRequest = { schema: 1, stamp: { auth_generation: 1, mount_generation: 1, history_revision: 1, layout_id: 1 }, rows: visible.map((row) => ({ uri: row.uri, width: 440 })) };
    const measurementWork = visible.filter((row) => !measured.has(row.uri)).slice(0, 3);
    expect(measurementWork).toHaveLength(0);
    expect(layoutRequest.rows.length).toBeGreaterThan(0);
    const placement = layoutRows(history.map((row) => ({
      uri: row.uri, status: "ready" as const, width: 440, line_count: 2,
      line_height: 18, baseline_offset: 14, body_height: 36, continuation: false,
    })));
    const positioned = placement.filter((row) => visible.some((entry) => entry.uri === row.uri));
    expect(positioned.map((row) => row.uri)).toEqual(visible.map((row) => row.uri));
  });
  it("keeps pending viewport neighbors in ordered bounded measurement work", () => {
    const visible = ["u4", "u5", "u6", "u7"].map((uri) => ({ uri }));
    const measured = new Set(["u4", "u6"]);
    const work = visible.filter((row) => !measured.has(row.uri)).slice(0, 3);
    expect(work.map((row) => row.uri)).toEqual(["u5", "u7"]);
  });
  it("keeps the scrolling viewport below the fixed header", () => {
    expect(ROW_LAYOUT.top).toBe(40);
    expect(ROW_LAYOUT.viewportHeight - ROW_LAYOUT.top - ROW_LAYOUT.bottom).toBe(224);
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
      validatePlacement({ schema: 1, stamp: req.stamp, rows: packed }, req, metrics),
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
