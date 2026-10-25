import { createSignal } from "solid-js";
import {
  ActionHandler,
  Text,
  View,
} from "@pocketjs/framework/solid/components";
import { BTN } from "@pocketjs/framework/input";
import {
  FeedDelivery,
  parseDelivery,
  requestFeed,
  takeFeedCommands,
} from "./feed";
import {
  firstCandidates,
  layoutRows,
  scrollWindow,
  preserveAnchorOffset,
  ROW_LAYOUT,
  moveSelection,
  reduceTimelineScroll,
  reconcileSelection,
  visibleWindow,
  parseMeasureRequest,
  validateReply,
  type MeasureReply,
  type MeasureRequest,
  type Stamp,
} from "./visible_text";
import { For } from "solid-js";
export function Timeline() {
  const [delivery, setDelivery] = createSignal<FeedDelivery | null>(null);
  const [error, setError] = createSignal("");
  const [layout, setLayout] = createSignal<ReturnType<typeof layoutRows>>([]);
  const [selectedUri, setSelectedUri] = createSignal<string | null>(null);
  const [scrollOffset, setScrollOffset] = createSignal(0);
  const heights = new Map<string, number>();
  const metricsByUri = new Map<string, import("./visible_text").Metrics>();
  const measured = new Set<string>();
  let active: MeasureRequest | null = null;
  let currentStamp: Stamp | null = null;
  const historyMetrics = (history: FeedDelivery["posts"]) => history.flatMap((post) => {
    const cached = metricsByUri.get(post.uri);
    return cached
      ? [{ ...cached, context: post.state.state === "clean" ? (post.context ?? null) : null }]
      : [];
  });
  const viewportLayout = (history: FeedDelivery["posts"]) => layoutRows(history.map((post) => {
    const metric = metricsByUri.get(post.uri);
    return metric
      ? { ...metric, context: post.state.state === "clean" ? (post.context ?? null) : null }
      : { uri: post.uri, status: "placeholder" as const, width: 440, line_count: 0, line_height: 0, baseline_offset: 0, body_height: 0, continuation: false, context: null };
  }));
  const publishViewport = (history: FeedDelivery["posts"], offset: number, stamp: Stamp) => {
    const visible = scrollWindow(history, heights, offset, ROW_LAYOUT.viewportHeight - ROW_LAYOUT.top - ROW_LAYOUT.bottom, 64).rows;
    const placementRequest = { schema: 1 as const, stamp, rows: visible.map((row) => ({ uri: row.uri, width: 440 })) };
    globalThis.__bskyVisiblePlacementRequest = JSON.stringify(placementRequest);
    const measuredRequest = {
      ...placementRequest,
      rows: placementRequest.rows.filter((row) => measured.has(row.uri)),
    };
    globalThis.__bskyVisibleLayoutRequest = JSON.stringify(measuredRequest);
    const positioned = viewportLayout(history);
    globalThis.__bskyVisiblePlacement = JSON.stringify({
      schema: 1,
      stamp,
      rows: positioned
        .filter((row) => visible.some((entry) => entry.uri === row.uri) && measured.has(row.uri))
        .map((row) => ({
          uri: row.uri,
          x: row.body_x,
          y: row.body_y - offset,
          body_height: row.body_height,
        })),
    });
    globalThis.__bskyVisibleMetrics = JSON.stringify(historyMetrics(history).map(({ uri, status, width, line_count, line_height, baseline_offset, body_height, continuation }) => ({
      uri, status, width, line_count, line_height, baseline_offset, body_height, continuation,
    })));
    const missing = visible.filter((row) => !measured.has(row.uri)).slice(0, 3);
    globalThis.__bskyVisibleWindow = missing.length
      ? JSON.stringify({ schema: 1, stamp, rows: missing.map((row) => ({ uri: row.uri, width: 440 })) })
      : "";
  };
  globalThis.__bskyTakeFeedCommands = () => JSON.stringify(takeFeedCommands());
  globalThis.__bskySetFeedError = (m) => setError(m.slice(0, 160));
  requestFeed("first_page");
  globalThis.__bskyReceiveFeed = (raw) => {
    const next = parseDelivery(raw);
    if (!next) {
      setError("Feed data unavailable");
      return;
    }
    setError("");
    const previous = delivery()?.posts ?? [];
    const selection = reconcileSelection(previous, next.posts, selectedUri());
    setSelectedUri(selection);
    const oldHeights = new Map(heights);
    const nextUris = new Set(next.posts.map((post) => post.uri));
    for (const uri of heights.keys()) if (!nextUris.has(uri)) { heights.delete(uri); metricsByUri.delete(uri); measured.delete(uri); }
    const anchor = selectedUri();
    const adjusted = preserveAnchorOffset(next.posts, oldHeights, heights, anchor, scrollOffset());
    setScrollOffset(adjusted);
    setDelivery(next);
    setLayout((old) => old.filter((row) => nextUris.has(row.uri)));
    currentStamp = null;
    globalThis.__bskyVisibleWindow = "";
    globalThis.nativeBatch.commands = [];
    if (selection) {
      const rows = scrollWindow(next.posts, heights, adjusted, ROW_LAYOUT.viewportHeight - ROW_LAYOUT.top - ROW_LAYOUT.bottom, 64).rows.slice(0, 3).map((row) => ({
        uri: row.uri,
        width: 440,
      }));
      const revision = Number(globalThis.__bskyHistoryRevision ?? 0);
      if (Number.isSafeInteger(revision) && revision > 0) {
        globalThis.__bskyVisibleWindow = JSON.stringify({
          schema: 1,
          stamp: {
            auth_generation: Number(globalThis.__bskyAuthGeneration ?? 1),
            mount_generation: Number(globalThis.__bskyMountGeneration ?? 1),
            history_revision: revision,
            layout_id: 1,
          },
          rows,
        });
      }
    }
  };
  globalThis.__bskyVisibleCandidates = (raw) => {
    const req = parseMeasureRequest(raw);
    if (!req || req.rows.length === 0) return "";
    const history = delivery()?.posts ?? [];
    if (req.rows.some((row) => !history.some((post) => post.uri === row.uri))) return "";
    const candidate = req.rows.map((row) =>
      history.find((post) => post.uri === row.uri),
    );
    if (candidate.some((p) => !p)) return "";
    currentStamp = req.stamp;
    active = req;
    // Keep the active measurement request separate from the native viewport
    // request: cached rows may be placed even when they were not candidates.
    const visible = geometry().rows;
    globalThis.__bskyVisibleLayoutRequest = JSON.stringify({
      schema: 1,
      stamp: req.stamp,
      rows: visible.map((row) => ({ uri: row.uri, width: 440 })),
    });
    const positioned = viewportLayout(history);
    const cachedRows = positioned.filter((row) => visible.some((entry) => entry.uri === row.uri) && measured.has(row.uri));
    globalThis.__bskyVisiblePlacement = JSON.stringify({
      schema: 1,
      stamp: req.stamp,
      rows: cachedRows.map((row) => ({ uri: row.uri, x: row.body_x, y: row.body_y - scrollOffset(), body_height: row.body_height })),
    });
    return JSON.stringify({
      schema: 1,
      stamp: req.stamp,
      rows: candidate.map((p, i) => ({
        uri: p!.uri,
        width: req.rows[i]!.width,
        text: p!.state.state === "clean" ? p!.text : "",
        status: p!.state.state === "clean" ? "ready" : "placeholder",
      })),
    });
  };
  globalThis.__bskyVisibleMeasured = (raw) => {
    if (!active) return "";
    try {
      const reply = JSON.parse(raw) as MeasureReply;
      if (!validateReply(reply, active)) {
        active = null;
        return "";
      }
      const history = delivery()?.posts ?? [];
      const beforeHeights = new Map(heights);
      for (const metric of reply.rows) { metricsByUri.set(metric.uri, metric); measured.add(metric.uri); }
      const combined = viewportLayout(history);
      for (const row of combined) heights.set(row.uri, row.height);
      setScrollOffset(preserveAnchorOffset(history, beforeHeights, heights, selectedUri(), scrollOffset()));
      const { offset, rows: visible } = geometry();
      globalThis.__bskyVisibleLayoutRequest = JSON.stringify({
        schema: 1,
        stamp: reply.stamp,
        rows: visible.map((row) => ({ uri: row.uri, width: 440 })),
      });
      const knownMetrics = historyMetrics(history);
      const measuredVisible = visible.filter((row) => measured.has(row.uri));
      globalThis.__bskyVisiblePlacementRequest = JSON.stringify({
        schema: 1,
        stamp: reply.stamp,
        rows: visible.map((row) => ({ uri: row.uri, width: 440 })),
      });
      globalThis.__bskyVisibleLayoutRequest = globalThis.__bskyVisiblePlacementRequest;
      globalThis.__bskyVisibleWindow = JSON.stringify({
        schema: 1,
        stamp: reply.stamp,
        rows: visible.filter((row) => !measured.has(row.uri)).slice(0, 3).map((row) => ({ uri: row.uri, width: 440 })),
      });
      if (!visible.some((row) => !measured.has(row.uri))) {
        globalThis.__bskyVisibleWindow = "";
      }
      globalThis.__bskyVisiblePlacement = JSON.stringify({
        schema: 1,
        stamp: reply.stamp,
        rows: combined.filter((row) => measuredVisible.some((entry) => entry.uri === row.uri)).map((r) => ({ uri: r.uri, x: r.body_x, y: r.body_y - offset, body_height: r.body_height })),
      });
      globalThis.__bskyVisibleMetrics = JSON.stringify(knownMetrics.map(({ uri, status, width, line_count, line_height, baseline_offset, body_height, continuation }) => ({
        uri, status, width, line_count, line_height, baseline_offset, body_height, continuation,
      })));
      setLayout(combined);
    } catch {
      globalThis.__bskyVisiblePlacement = "";
    }
    active = null;
    return "";
  };
  const geometry = () => scrollWindow(delivery()?.posts ?? [], heights, scrollOffset(), ROW_LAYOUT.viewportHeight - ROW_LAYOUT.top, 64);
  const posts = () => {
    const history = delivery()?.posts ?? [];
    const set = new Set(geometry().rows.map((row) => row.uri));
    return history.filter((post) => set.has(post.uri));
  };
  const publishSelection = (delta: -1 | 1) => {
    const history = delivery()?.posts ?? [];
    const next = reduceTimelineScroll(history, heights, { offset: scrollOffset(), selected: selectedUri() }, delta);
    const selected = next.selected;
    setSelectedUri(selected);
    setScrollOffset(next.offset);
    if (currentStamp) {
      publishViewport(history, next.offset, currentStamp);
    }
  };
  const selectPrevious = () => publishSelection(-1);
  const selectNext = () => publishSelection(1);
  const status = () =>
    error() ||
    (!delivery()
      ? "Loading following feed…"
      : posts().length
        ? ""
        : "No posts yet");
  return (
    <View class="relative w-full h-full bg-slate-950">
      <Text
        class="absolute text-lg text-white font-bold"
        style={{ insetL: 12, insetT: 8, height: 22 }}
      >
        Following
      </Text>
      {status() && (
        <Text
          class="absolute text-xs text-slate-400"
          style={{ insetL: 12, insetT: 30, height: 12 }}
        >
          {status()}
        </Text>
      )}
      <View class="absolute overflow-hidden" style={{ insetL: 0, insetT: 40, width: 480, height: 224 }}>
      <For each={posts()}>
        {(post, index) => {
          const rect = () => layout().find((row) => row.uri === post.uri);
          const top = () => (rect()?.y ?? 40 + index() * 92) - geometry().offset - ROW_LAYOUT.top;
          const contextHeight = () => rect()?.context_height ?? 0;
          const bodyY = () => (rect()?.body_y ?? top() + 42) - geometry().offset - ROW_LAYOUT.top;
          const cardHeight = () => rect()?.height ?? 84;
          const bodyHeight = () => rect()?.body_height ?? 18;
          const pending = () => !metricsByUri.has(post.uri);
          const x = () => rect()?.x ?? 12;
          const width = () => rect()?.width ?? 456;
          return (
            <>
              <View
                class="absolute rounded-lg bg-slate-900"
                style={{
                  insetL: x(),
                  insetT: top(),
                  width: width(),
                  height: cardHeight(),
                }}
              />
              <Text
                class="absolute text-sm text-white font-bold"
                style={{
                  insetL: x() + 8,
                  insetT: top() + 8,
                  width: width() - 16,
                  height: 22,
                }}
              >
                {post.state.state === "clean"
                  ? (post.author.display_name || post.author.handle) +
                    " · @" +
                    post.author.handle
                  : "Content unavailable"}
              </Text>
              {post.state.state === "clean" && post.context && (
                <Text
                  class="absolute text-xs text-sky-300"
                  style={{
                    insetL: x() + 8,
                    insetT: bodyY() - contextHeight(),
                    width: width() - 16,
                    height: 12,
                  }}
                >
                  {
                    {
                      reply: "Reply",
                      repost: "Reposted",
                      quote: "Quote",
                      external: "External link",
                      video: "Video",
                      unsupported: "Unsupported attachment",
                    }[post.context]
                  }
                </Text>
              )}
              {post.state.state === "clean" && pending() && (
                <Text class="absolute text-xs text-slate-400" style={{ insetL: x() + 8, insetT: bodyY(), width: width() - 16, height: 18 }}>Loading…</Text>
              )}
              {post.state.state === "clean" && !pending() && (
                <Text
                  class="absolute text-xs text-slate-200"
                  style={{
                    insetL: x() + 8,
                    insetT: bodyY(),
                    width: width() - 16,
                    height: bodyHeight(),
                  }}
                />
              )}
              <Text
                class="absolute text-xs text-slate-400"
                style={{
                  insetL: x() + 8,
                  insetT: bodyY() + bodyHeight() + 4,
                  width: width() - 16,
                  height: 12,
                }}
              >
                {post.state.state === "clean"
                  ? `♥ ${post.like_count}  ↻ ${post.repost_count}  ◌ ${post.reply_count}`
                  : ""}
              </Text>
            </>
          );
        }}
      </For>
      </View>
      <ActionHandler button={BTN.UP} onPress={selectPrevious} />
      <ActionHandler button={BTN.DOWN} onPress={selectNext} />
      <ActionHandler
        button={BTN.CROSS}
        onPress={() =>
          requestFeed(
            error() ? "retry" : delivery()?.cursor ? "next_page" : "refresh",
          )
        }
      />
      <ActionHandler
        button={BTN.TRIANGLE}
        onPress={() => requestFeed("refresh")}
      />
      {delivery()?.cursor && (
        <Text class="text-xs text-sky-300">More posts available · Cross</Text>
      )}
      <Text class="text-xs text-sky-300">Select · Up/Down</Text>
      <Text class="text-xs text-sky-300">Refresh · Triangle</Text>
      {error() && <Text class="text-xs text-sky-300">Retry · Cross</Text>}
    </View>
  );
}
declare global {
  var __bskyVisibleWindow: string;
  var __bskyAuthGeneration: number;
  var __bskyMountGeneration: number;
  var __bskyHistoryRevision: number;
  var __bskyVisibleCandidates: (json: string) => string;
  var __bskyVisibleMeasured: (json: string) => string;
  var __bskyVisiblePlacement: string;
  var __bskyVisibleMetrics: string;
  var __bskyVisiblePlacementRequest: string;
  var __bskyVisibleLayoutRequest: string;
}
