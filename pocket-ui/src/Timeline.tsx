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
  moveSelection,
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
  let active: MeasureRequest | null = null;
  let currentStamp: Stamp | null = null;
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
    setDelivery(next);
    setLayout([]);
    currentStamp = null;
    globalThis.__bskyVisibleWindow = "";
    globalThis.nativeBatch.commands = [];
    if (selection) {
      const rows = visibleWindow(next.posts, selection).map((post) => ({
        uri: post.uri,
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
    if (!req) return "";
    const history = delivery()?.posts ?? [];
    const candidate = req.rows.map((row) =>
      history.find((post) => post.uri === row.uri),
    );
    if (candidate.some((p) => !p)) return "";
    currentStamp = req.stamp;
    active = req;
    globalThis.__bskyVisiblePlacement = "";
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
      const selection = JSON.parse(globalThis.__bskyVisibleWindow || "null") as { schema: number; stamp: Stamp; rows: { uri: string; width: number }[] } | null;
      if (
        !validateReply(reply, active) ||
        (selection && selection.stamp.auth_generation === reply.stamp.auth_generation &&
          selection.stamp.mount_generation === reply.stamp.mount_generation &&
          selection.stamp.history_revision === reply.stamp.history_revision &&
          selection.rows.some((row, index) => row.uri !== reply.rows[index]?.uri))
      ) {
        active = null;
        return "";
      }
      const history = delivery()?.posts ?? [];
      const rows = layoutRows(
        reply.rows.map((row) => {
          const post = history.find((entry) => entry.uri === row.uri);
          return {
            ...row,
            context:
              post?.state.state === "clean" ? (post.context ?? null) : null,
          };
        }),
      );
      const next = JSON.stringify({
        schema: 1,
        stamp: reply.stamp,
        rows: rows.map((r) => ({
          uri: r.uri,
          x: r.body_x,
          y: r.body_y,
          body_height: r.body_height,
        })),
      });
      globalThis.__bskyVisiblePlacement = next;
      setLayout(rows);
    } catch {
      globalThis.__bskyVisiblePlacement = "";
    }
    active = null;
    return "";
  };
  const posts = () => visibleWindow(delivery()?.posts ?? [], selectedUri());
  const publishSelection = (delta: -1 | 1) => {
    const history = delivery()?.posts ?? [];
    const selected = moveSelection(history, selectedUri(), delta);
    setSelectedUri(selected);
    if (selected && currentStamp) {
      const rows = visibleWindow(history, selected).map((post) => ({
        uri: post.uri,
        width: 440,
      }));
      globalThis.__bskyVisibleWindow = JSON.stringify({ schema: 1, stamp: currentStamp, rows });
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
    <View class="relative w-full h-full overflow-hidden bg-slate-950">
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
      <For each={posts()}>
        {(post, index) => {
          const rect = () => layout().find((row) => row.uri === post.uri);
          const top = () => rect()?.y ?? 40 + index() * 92;
          const contextHeight = () => rect()?.context_height ?? 0;
          const bodyY = () => rect()?.body_y ?? top() + 42;
          const cardHeight = () => rect()?.height ?? 84;
          const bodyHeight = () => rect()?.body_height ?? 18;
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
              {post.state.state === "clean" && (
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
}
