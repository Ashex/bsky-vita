export type Stamp = {
  auth_generation: number;
  mount_generation: number;
  history_revision: number;
  layout_id: number;
};
export type ContextMarker =
  "reply" | "repost" | "quote" | "external" | "video" | "unsupported";
export type Candidate = { uri: string; width: number };
export type Metrics = {
  uri: string;
  status: "ready" | "placeholder";
  width: number;
  line_count: number;
  line_height: number;
  baseline_offset: number;
  body_height: number;
  continuation: boolean;
  context?: ContextMarker | null;
};
export type MeasureRequest = { schema: 1; stamp: Stamp; rows: Candidate[] };
export type MeasureReply = { schema: 1; stamp: Stamp; rows: Metrics[] };
export type Placement = {
  uri: string;
  x: number;
  y: number;
  body_height: number;
};
export type PlacementMessage = { schema: 1; stamp: Stamp; rows: Placement[] };
export const ROW_LAYOUT = {
  viewportWidth: 480,
  viewportHeight: 272,
  left: 12,
  top: 40,
  right: 12,
  bottom: 8,
  headerHeight: 22,
  headerGap: 4,
  contextHeight: 12,
  contextGap: 2,
  countHeight: 12,
  countGap: 4,
  horizontalPadding: 8,
  verticalPadding: 8,
  rowGap: 8,
  placeholderHeight: 18,
} as const;
export type CardLayout = {
  uri: string;
  x: number;
  y: number;
  width: number;
  height: number;
  body_x: number;
  body_y: number;
  body_height: number;
  context_height: number;
  card_height: number;
  index: number;
};
export function layoutRows(rows: Metrics[]): CardLayout[] {
  let y = ROW_LAYOUT.top;
  return rows.map((row, index) => {
    const width = Math.min(
      row.width + ROW_LAYOUT.horizontalPadding * 2,
      ROW_LAYOUT.viewportWidth - ROW_LAYOUT.left - ROW_LAYOUT.right,
    );
    const bodyHeight =
      row.status === "placeholder"
        ? ROW_LAYOUT.placeholderHeight
        : Math.max(1, row.body_height);
    const contextHeight =
      row.status === "ready" && row.context ? ROW_LAYOUT.contextHeight : 0;
    const contextGap = contextHeight > 0 ? ROW_LAYOUT.contextGap : 0;
    const height =
      ROW_LAYOUT.verticalPadding * 2 +
      ROW_LAYOUT.headerHeight +
      ROW_LAYOUT.headerGap +
      contextHeight +
      contextGap +
      bodyHeight +
      ROW_LAYOUT.countGap +
      ROW_LAYOUT.countHeight;
    const placement = {
      uri: row.uri,
      x: ROW_LAYOUT.left,
      y,
      width,
      height,
      body_x: ROW_LAYOUT.left + ROW_LAYOUT.horizontalPadding,
      body_y:
        y +
        ROW_LAYOUT.verticalPadding +
        ROW_LAYOUT.headerHeight +
        ROW_LAYOUT.headerGap +
        contextHeight +
        contextGap,
      body_height: bodyHeight,
      context_height: contextHeight,
      card_height: height,
      index,
    };
    y += height + ROW_LAYOUT.rowGap;
    return placement;
  });
}
export const MAX_MESSAGE_BYTES = 4096;
export const utf8Bytes = (s: string) => {
  let n = 0;
  for (const c of s)
    n +=
      c.codePointAt(0)! <= 0x7f
        ? 1
        : c.codePointAt(0)! <= 0x7ff
          ? 2
          : c.codePointAt(0)! <= 0xffff
            ? 3
            : 4;
  return n;
};
const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const keys = (v: Record<string, unknown>, expected: string[]) =>
  Object.keys(v).length === expected.length &&
  Object.keys(v).every((k) => expected.includes(k));
const stampOk = (v: unknown): v is Stamp =>
  record(v) &&
  keys(v, [
    "auth_generation",
    "mount_generation",
    "history_revision",
    "layout_id",
  ]) &&
  Object.values(v).every((n) => Number.isSafeInteger(n) && (n as number) > 0);
export function parseMeasureRequest(raw: string): MeasureRequest | null {
  try {
    if (utf8Bytes(raw) > MAX_MESSAGE_BYTES) return null;
    const v: unknown = JSON.parse(raw);
    if (
      !record(v) ||
      !keys(v, ["schema", "stamp", "rows"]) ||
      v.schema !== 1 ||
      !stampOk(v.stamp) ||
      !Array.isArray(v.rows) ||
      v.rows.length > 3
    )
      return null;
    let total = 0;
    for (const row of v.rows) {
      if (
        !record(row) ||
        !keys(row, ["uri", "width"]) ||
        typeof row.uri !== "string" ||
        !row.uri ||
        utf8Bytes(row.uri) > 1024 ||
        !Number.isInteger(row.width) ||
        (row.width as number) < 32 ||
        (row.width as number) > 440
      )
        return null;
      total += utf8Bytes(row.uri);
    }
    if (total > 3072) return null;
    return v as unknown as MeasureRequest;
  } catch {
    return null;
  }
}
export function validateReply(
  reply: MeasureReply,
  request: MeasureRequest,
): boolean {
  return (
    reply.schema === 1 &&
    reply.stamp.auth_generation === request.stamp.auth_generation &&
    reply.stamp.mount_generation === request.stamp.mount_generation &&
    reply.stamp.history_revision === request.stamp.history_revision &&
    reply.stamp.layout_id === request.stamp.layout_id &&
    reply.rows.length >= 1 &&
    reply.rows.length <= request.rows.length &&
    reply.rows.every((r, i) => {
      const c = request.rows[i];
      return (
        !!c &&
        r.uri === c.uri &&
        r.width === c.width &&
        (r.status === "ready" || r.status === "placeholder") &&
        Number.isInteger(r.line_count) &&
        r.line_count >= 0 &&
        r.line_count <= 65536 &&
        [r.line_height, r.baseline_offset, r.body_height].every(
          (n) => Number.isSafeInteger(n) && n >= 0,
        ) &&
        typeof r.continuation === "boolean"
      );
    })
  );
}
export function validatePlacement(
  p: PlacementMessage,
  request: MeasureRequest,
  known: Metrics[] = [],
): boolean {
  const indices = p.rows.map((row) => request.rows.findIndex((candidate) => candidate.uri === row.uri));
  return (
    p.schema === 1 &&
    JSON.stringify(p.stamp) === JSON.stringify(request.stamp) &&
    p.rows.length > 0 && p.rows.length === request.rows.length && p.rows.length <= 200 &&
    p.rows.every((r, i) => {
      const metric = known.find((entry) => entry.uri === r.uri);
      return indices[i]! >= 0 && (i === 0 || indices[i]! > indices[i - 1]!) &&
        !!metric && metric.body_height === r.body_height &&
        [r.x, r.y, r.body_height].every(Number.isSafeInteger) &&
        r.x >= 0 && r.x <= 440 && r.body_height > 0 && r.body_height <= 1_000_000;
    })
  );
}
export function visibleWindow<T extends { uri: string }>(
  history: T[],
  selected: string | null,
  limit = 3,
): T[] {
  if (!history.length || limit <= 0) return [];
  const selectedIndex = history.findIndex((post) => post.uri === selected);
  const start = Math.max(0, selectedIndex);
  return history.slice(start, Math.min(history.length, start + limit));
}
export type HeightEntry = { uri: string; height: number };
export type TimelineScrollState = { offset: number; selected: string | null };
export type TouchContact = { id: number; x: number; y: number };
export type TouchRailState =
  | { kind: "idle" }
  | { kind: "blocked" }
  | { kind: "captured"; id: number; lastY: number; dragging: boolean };
export const TOUCH_RAIL = { left: 456, right: 480, top: 40, bottom: 264 } as const;
const IDLE_TOUCH: TouchRailState = { kind: "idle" };
const BLOCKED_TOUCH: TouchRailState = { kind: "blocked" };
export function reduceTouchRail(
  state: TouchRailState | null,
  contacts: TouchContact[],
): { state: TouchRailState; delta: number } {
  const current = state ?? IDLE_TOUCH;
  if (current.kind === "blocked") {
    return contacts.length ? { state: BLOCKED_TOUCH, delta: 0 } : { state: IDLE_TOUCH, delta: 0 };
  }
  if (contacts.length > 1) return { state: BLOCKED_TOUCH, delta: 0 };
  const contact = contacts[0];
  if (current.kind === "captured") {
    if (!contact || contact.id !== current.id) {
      return { state: contacts.length ? BLOCKED_TOUCH : IDLE_TOUCH, delta: 0 };
    }
    const delta = current.lastY - contact.y;
    return { state: { kind: "captured", id: current.id, lastY: contact.y, dragging: current.dragging || delta !== 0 }, delta };
  }
  if (!contact) return { state: IDLE_TOUCH, delta: 0 };
  if (contact.x < TOUCH_RAIL.left || contact.x >= TOUCH_RAIL.right || contact.y < TOUCH_RAIL.top || contact.y >= TOUCH_RAIL.bottom) {
    return { state: BLOCKED_TOUCH, delta: 0 };
  }
  return { state: { kind: "captured", id: contact.id, lastY: contact.y, dragging: false }, delta: 0 };
}
export function reduceTimelineScroll(
  history: { uri: string }[], heights: ReadonlyMap<string, number>,
  state: TimelineScrollState, delta: -1 | 1, viewportHeight = ROW_LAYOUT.viewportHeight - ROW_LAYOUT.top - ROW_LAYOUT.bottom,
): TimelineScrollState {
  if (!history.length) return { offset: 0, selected: null };
  const window = scrollWindow(history, heights, state.offset + delta * 24, viewportHeight);
  const selected = moveSelection(history, state.selected, delta);
  return { offset: window.offset, selected };
}
export type ScrollWindow = { rows: HeightEntry[]; offset: number; totalHeight: number };
export function fullCardHeight(uri: string, heights: ReadonlyMap<string, number>): number {
  return Math.max(1, Math.min(1_000_000, heights.get(uri) ?? ROW_LAYOUT.verticalPadding * 2 + ROW_LAYOUT.headerHeight + ROW_LAYOUT.headerGap + ROW_LAYOUT.countGap + ROW_LAYOUT.countHeight + 18));
}
export function scrollWindow(
  history: { uri: string }[],
  heights: ReadonlyMap<string, number>,
  offset: number,
  viewportHeight: number,
  overscan = 64,
): ScrollWindow {
  const rowHeight = (uri: string) => fullCardHeight(uri, heights);
  const totalHeight = history.reduce((sum, row) => sum + rowHeight(row.uri) + ROW_LAYOUT.rowGap, 0);
  const maxOffset = Math.max(0, totalHeight - viewportHeight);
  const clamped = Math.max(0, Math.min(maxOffset, Number.isFinite(offset) ? offset : 0));
  const begin = Math.max(0, clamped - overscan);
  const end = clamped + viewportHeight + overscan;
  let y = 0;
  const rows: HeightEntry[] = [];
  for (const row of history) {
    const height = rowHeight(row.uri);
    if (y + height >= begin && y <= end) rows.push({ uri: row.uri, height });
    y += height + ROW_LAYOUT.rowGap;
    if (y > end) break;
  }
  return { rows, offset: clamped, totalHeight };
}
export function preserveAnchorOffset(
  history: { uri: string }[],
  before: ReadonlyMap<string, number>,
  after: ReadonlyMap<string, number>,
  anchorUri: string | null,
  offset: number,
): number {
  if (!anchorUri) return offset;
  const index = history.findIndex((row) => row.uri === anchorUri);
  if (index < 0) return offset;
  let delta = 0;
  for (let i = 0; i < index; i++) {
    const uri = history[i]!.uri;
    delta += fullCardHeight(uri, after) - fullCardHeight(uri, before);
  }
  return Math.max(0, offset + delta);
}
export function moveSelection(
  history: { uri: string }[],
  selected: string | null,
  delta: -1 | 1,
): string | null {
  if (!history.length) return null;
  const index = history.findIndex((post) => post.uri === selected);
  return history[Math.max(0, Math.min(history.length - 1, (index < 0 ? 0 : index) + delta))]!.uri;
}
export function reconcileSelection(
  previous: { uri: string }[],
  next: { uri: string }[],
  selected: string | null,
): string | null {
  if (!next.length) return null;
  const retained = selected && next.some((post) => post.uri === selected);
  if (retained) return selected;
  const previousIndex = previous.findIndex((post) => post.uri === selected);
  return next[Math.max(0, Math.min(previousIndex < 0 ? 0 : previousIndex, next.length - 1))]!.uri;
}
export function firstCandidates<T extends { uri: string }>(history: T[]): T[] {
  return visibleWindow(history, history[0]?.uri ?? null);
}
