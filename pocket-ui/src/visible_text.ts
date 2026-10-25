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
};
export function layoutRows(rows: Metrics[]): CardLayout[] {
  let y = ROW_LAYOUT.top;
  return rows.map((row) => {
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
      v.rows.length < 1 ||
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
    JSON.stringify(reply.stamp) === JSON.stringify(request.stamp) &&
    reply.rows.length === request.rows.length &&
    reply.rows.every((r, i) => {
      const c = request.rows[i];
      return (
        !!c &&
        r.uri === c.uri &&
        r.width === c.width &&
        (r.status === "ready" || r.status === "placeholder") &&
        Number.isInteger(r.line_count) &&
        r.line_count >= 0 &&
        r.line_count <= 2 &&
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
): boolean {
  return (
    p.schema === 1 &&
    JSON.stringify(p.stamp) === JSON.stringify(request.stamp) &&
    p.rows.length === request.rows.length &&
    p.rows.every(
      (r, i) =>
        r.uri === request.rows[i]?.uri &&
        [r.x, r.y, r.body_height].every(Number.isInteger) &&
        r.x >= 0 &&
        r.y >= 0 &&
        r.body_height > 0 &&
        r.body_height <= 512,
    )
  );
}
export function visibleWindow<T extends { uri: string }>(
  history: T[],
  selected: string | null,
): T[] {
  if (!history.length) return [];
  const selectedIndex = history.findIndex((post) => post.uri === selected);
  const index = Math.max(0, selectedIndex);
  const start = Math.min(index, Math.max(0, history.length - 3));
  return history.slice(start, start + 3);
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
