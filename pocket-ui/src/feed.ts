export type ContextMarker = "reply" | "repost" | "quote" | "external" | "video" | "unsupported";
export type ContentState = { state: "clean" | "blocked" | "muted" | "unavailable" | "unknown" } | { state: "warning"; detail: string };
export type FeedPost = { uri: string; cid: string; author: { did: string; handle: string; display_name: string }; text: string; like_count: number; repost_count: number; reply_count: number; state: ContentState; images: unknown[]; quote?: FeedPost | null; context?: ContextMarker | null };
export type FeedDelivery = { schema: number; correlation: { auth_generation: number; mount_generation: number; request_id: number }; posts: FeedPost[]; cursor: string | null };

const MAX_DELIVERY_BYTES = 1024 * 1024;
const MAX_FIELD_BYTES = 64 * 1024;
const MAX_POST_BYTES = 64 * 1024;
const MAX_POSTS = 200;

// QuickJS' guest profile does not provide TextEncoder. Count UTF-8 bytes
// directly, treating valid surrogate pairs as one code point and lone
// surrogates as U+FFFD (matching TextEncoder's replacement behavior).
export function utf8ByteLength(value: string): number {
  let bytes = 0;
  for (let i = 0; i < value.length; i++) {
    const unit = value.charCodeAt(i);
    if (unit <= 0x7f) bytes += 1;
    else if (unit <= 0x7ff) bytes += 2;
    else if (unit >= 0xd800 && unit <= 0xdbff && i + 1 < value.length && value.charCodeAt(i + 1) >= 0xdc00 && value.charCodeAt(i + 1) <= 0xdfff) {
      bytes += 4;
      i++;
    } else if (unit >= 0xd800 && unit <= 0xdfff) bytes += 3;
    else bytes += 3;
  }
  return bytes;
}

export function parseDelivery(raw: string): FeedDelivery | null {
  try {
    if (utf8ByteLength(raw) > MAX_DELIVERY_BYTES) return null;
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value) || value.schema !== 1 || !Array.isArray(value.posts) || value.posts.length > MAX_POSTS || !isRecord(value.correlation)) return null;
    const correlation = value.correlation;
    if (correlation.schema !== 1 || ![correlation.auth_generation, correlation.mount_generation, correlation.request_id].every(n => Number.isSafeInteger(n) && n > 0)) return null;
    if (Object.keys(correlation).length !== 6 || Object.keys(correlation).some(key => !["schema", "auth_generation", "mount_generation", "request_id", "source", "cursor"].includes(key))) return null;
    if (typeof correlation.source !== "string" || utf8ByteLength(correlation.source) > MAX_FIELD_BYTES || (typeof correlation.cursor !== "string" && correlation.cursor !== null) || (correlation.cursor !== null && utf8ByteLength(correlation.cursor) > MAX_FIELD_BYTES)) return null;
    if (Object.keys(value).length !== 4 || Object.keys(value).some(key => !["schema", "correlation", "posts", "cursor"].includes(key))) return null;
    if (typeof value.cursor !== "string" && value.cursor !== null) return null;
    if (value.cursor !== null && utf8ByteLength(value.cursor) > MAX_FIELD_BYTES) return null;
    if (!value.posts.every(post => isPost(post, 0))) return null;
    return value as FeedDelivery;
  } catch { return null; }
}

function isRecord(v: unknown): v is Record<string, any> { return typeof v === "object" && v !== null && !Array.isArray(v); }
function isState(v: unknown): v is ContentState {
  if (!isRecord(v)) return false;
  if (v.state === "warning") return Object.keys(v).length === 2 && Object.keys(v).every(key => key === "state" || key === "detail") && typeof v.detail === "string" && utf8ByteLength(v.detail) <= MAX_FIELD_BYTES;
  return Object.keys(v).length === 1 && ["clean", "blocked", "muted", "unavailable", "unknown"].includes(v.state);
}
function isPost(v: unknown, depth: number): v is FeedPost {
  if (!isRecord(v) || depth >= 16 || !isRecord(v.author) || !Array.isArray(v.images) || !Array.isArray(v.facets ?? []) || !isState(v.state)) return false;
  if (["uri", "cid", "author", "text", "facets", "like_count", "repost_count", "reply_count", "state", "quote", "images"].some(key => !(key in v))) return false;
  if (Object.keys(v).some(key => !["uri", "cid", "author", "text", "facets", "like_count", "repost_count", "reply_count", "state", "quote", "images", "context"].includes(key))) return false;
  if (v.context !== undefined && v.context !== null && !["reply", "repost", "quote", "external", "video", "unsupported"].includes(v.context)) return false;
  if (v.state.state !== "clean" && v.context !== undefined) return false;
  if (Object.keys(v.author).some(key => !["did", "handle", "display_name"].includes(key))) return false;
  if (v.state.state !== "clean" && (v.text !== "" || v.images.length !== 0 || (v.quote !== undefined && v.quote !== null))) return false;
  if (v.facets.some((facet: unknown) => !isRecord(facet) || !Number.isSafeInteger(facet.start) || !Number.isSafeInteger(facet.end) || typeof facet.feature !== "string" || facet.start < 0 || facet.end <= facet.start || facet.end > utf8ByteLength(v.text ?? ""))) return false;
  if (v.images.some((image: unknown) => !isRecord(image) || typeof image.alt !== "string" || utf8ByteLength(image.alt) > MAX_FIELD_BYTES)) return false;
  for (const key of ["uri", "cid", "text"]) if (typeof v[key] !== "string" || utf8ByteLength(v[key]) > MAX_FIELD_BYTES) return false;
  for (const key of ["did", "handle", "display_name"]) if (typeof v.author[key] !== "string" || utf8ByteLength(v.author[key]) > MAX_FIELD_BYTES) return false;
  if (![v.like_count, v.repost_count, v.reply_count].every(n => Number.isSafeInteger(n) && n >= 0)) return false;
  if (utf8ByteLength(JSON.stringify(v)) > MAX_POST_BYTES) return false;
  if (v.quote !== undefined && v.quote !== null && !isPost(v.quote, depth + 1)) return false;
  return true;
}

export type FeedCommand = "first_page" | "next_page" | "refresh" | "retry" | "leave";
const COMMANDS = new Set<FeedCommand>(["first_page", "next_page", "refresh", "retry", "leave"]);
const pendingCommands: FeedCommand[] = [];
let feedError = "";
export function takeFeedCommands(): FeedCommand[] { return pendingCommands.splice(0, 128); }
export function setFeedError(message: string): void { feedError = message.slice(0, 160); }
export function currentFeedError(): string { return feedError; }
export function requestFeed(command: string): boolean {
  if (!COMMANDS.has(command as FeedCommand)) return false;
  if (pendingCommands.length >= 128) return false;
  pendingCommands.push(command as FeedCommand);
  return true;
}

declare global {
  var __bskyRequestFeed: (command: string) => void;
  var __bskyTakeFeedCommands: () => string;
  var __bskySetFeedError: (message: string) => void;
}
