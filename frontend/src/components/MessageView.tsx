import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import Typewriter from "./Typewriter";
import type { StreamingReply } from "../App";

interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
}

interface EventRow {
  message_id: string;
  conversation_id: string;
  sender: string;
  sender_open_dingtalk_id: string;
  content: string;
  create_time: string;
  received_at: string;
  listen_kind: string;
  malformed: boolean;
  processed: boolean;
  reply_status: string | null;
  reply_text: string | null;
  /** 模型的思考过程（生成时存下来的整段）；没开思考或没回报就是 null。 */
  reasoning: string | null;
}

/** conversation_details 命令的返回里，会话窗口头部用得上的部分。 */
interface ConversationDetails {
  name: string;
  kind: string;
  context_usage_ratio: number | null;
  context_used_tokens: number | null;
  context_window_tokens: number | null;
  context_window_source: "session" | "default" | "none";
  compress_trigger_percent: number | null;
  model: string | null;
}

const KIND_LABEL: Record<string, string> = { group: "群聊", direct: "单聊" };

/** 列表刷新周期。生成中的块走监听频道即时到达，这里只是兜底对齐权威数据。 */
const POLL_MS = 3000;

/** 块超过这么久没再到达，就当作"已经不在生成了"，免得气泡永远转着。 */
const LIVE_TIMEOUT_MS = 20_000;

interface MessageViewProps {
  session: Session;
  /** 正在生成的块，按 message_id 归位；只用于观感，权威正文以落库事件为准。 */
  streamingByMessage: Record<string, StreamingReply>;
  /** 会话窗口头部的「⋯」里要用的操作，由 App 提供。 */
  onRefreshMeta: () => Promise<string>;
}

/** token 数按 k 显示：200000 → "200k"，64801 → "64.8k"。 */
function formatTokens(value: number) {
  if (value < 1000) return String(value);
  return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}

/** 折叠起来的思考过程：生成中自动展开，用户点过就听用户的。 */
function ThinkingBlock({
  text,
  live,
}: {
  text: string;
  live: boolean;
}) {
  const [open, setOpen] = useState(live);
  const touched = useRef(false);

  useEffect(() => {
    if (live && !touched.current) setOpen(true);
  }, [live]);

  const chars = text.replace(/\s/g, "").length;

  return (
    <div style={{ maxWidth: "78%", marginTop: "6px" }}>
      <button
        onClick={() => {
          touched.current = true;
          setOpen((value) => !value);
        }}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: "4px",
          padding: "2px 8px",
          fontSize: "11px",
          color: "var(--text-muted)",
          backgroundColor: "transparent",
          border: "1px solid var(--border)",
          borderRadius: "10px",
          cursor: "pointer",
        }}
        title={open ? "收起思考过程" : "展开思考过程"}
      >
        <span>{live ? "思考中" : "思考过程"}</span>
        <span>{open ? "▴" : "▾"}</span>
        {!open && <span>{chars} 字</span>}
      </button>
      {open && (
        <div
          style={{
            marginTop: "4px",
            padding: "8px 10px",
            borderRadius: "8px",
            borderLeft: "2px solid var(--border)",
            color: "var(--text-secondary)",
            fontSize: "12px",
            lineHeight: 1.6,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          <Typewriter text={text} showCaretWhenEmpty={live} />
        </div>
      )}
    </div>
  );
}

const bubbleBase: CSSProperties = {
  maxWidth: "78%",
  padding: "8px 12px",
  borderRadius: "8px",
  fontSize: "13px",
  lineHeight: 1.5,
  wordBreak: "break-word",
  whiteSpace: "pre-wrap",
};

/** 状态行文案：说清这条到底发出去了没有、没发出去是为什么。 */
function statusText(event: EventRow, live: boolean) {
  switch (event.reply_status) {
    case "sent":
      return { text: "✓ 已发送", color: "var(--success)" };
    case "skipped":
      return { text: `已跳过${event.reply_text ? `：${event.reply_text}` : ""}`, color: "var(--text-muted)" };
    case "failed":
      return { text: `回复失败${event.reply_text ? `：${event.reply_text}` : ""}`, color: "var(--danger)" };
    default:
      return live
        ? { text: "回复中…", color: "var(--warn)" }
        : { text: "等待回复…", color: "var(--text-muted)" };
  }
}

export default function MessageView({ session, streamingByMessage, onRefreshMeta }: MessageViewProps) {
  const [events, setEvents] = useState<EventRow[]>([]);
  const [details, setDetails] = useState<ConversationDetails | null>(null);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      setLoading(true);
      try {
        const [nextEvents, nextDetails] = await Promise.all([
          invoke<EventRow[]>("list_events", {
            projectId: session.project_id,
            conversationId: session.conversation_id,
            limit: 200,
          }),
          invoke<ConversationDetails>("conversation_details", {
            projectId: session.project_id,
            conversationId: session.conversation_id,
          }).catch(() => null),
        ]);
        if (!cancelled) {
          setEvents([...nextEvents].reverse());
          if (nextDetails) setDetails(nextDetails);
        }
      } catch (e) {
        console.error("Failed to load events:", e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    load();
    const timer = setInterval(load, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [session.project_id, session.conversation_id]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  const ratio = details?.context_usage_ratio ?? null;
  const trigger = details?.compress_trigger_percent ?? null;
  const overTrigger = ratio != null && trigger != null && ratio * 100 >= trigger;

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "8px",
          padding: "8px 16px",
          borderBottom: "1px solid var(--border)",
          backgroundColor: "var(--bg-sidebar)",
        }}
      >
        <span style={{ color: "var(--text-primary)", fontSize: "14px", fontWeight: 600 }}>
          {details?.name || session.name}
        </span>
        {details?.kind && (
          <span
            style={{
              padding: "1px 6px",
              fontSize: "11px",
              borderRadius: "8px",
              border: "1px solid var(--border)",
              color: details.kind === "group" ? "var(--accent)" : "var(--success)",
            }}
          >
            {KIND_LABEL[details.kind] ?? details.kind}
          </span>
        )}

        <div style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: "8px" }}>
          {ratio != null && (
            <>
              <div
                style={{
                  flex: 1,
                  minWidth: 0,
                  height: "4px",
                  borderRadius: "2px",
                  backgroundColor: "var(--bg-active)",
                  overflow: "hidden",
                }}
                title={`上下文占用 ${(ratio * 100).toFixed(2)}%${
                  trigger != null ? `（用到 ${trigger}% 自动压缩）` : ""
                }`}
              >
                <div
                  style={{
                    height: "100%",
                    width: `${Math.min(100, ratio * 100).toFixed(1)}%`,
                    backgroundColor: overTrigger ? "var(--warn)" : "var(--accent)",
                  }}
                />
              </div>
              <span
                style={{
                  fontSize: "11px",
                  color: overTrigger ? "var(--warn)" : "var(--text-muted)",
                  whiteSpace: "nowrap",
                }}
              >
                {(ratio * 100).toFixed(1)}%
                {details?.context_used_tokens != null && (
                  <> · {formatTokens(details.context_used_tokens)} tokens</>
                )}
                {details?.context_window_source === "default" && "（默认窗口）"}
              </span>
            </>
          )}
        </div>

        <button
          onClick={async () => {
            try {
              setNotice(await onRefreshMeta());
            } catch (e) {
              setNotice(`同步失败：${e}`);
            }
          }}
          style={{
            padding: "2px 8px",
            fontSize: "11px",
            color: "var(--text-secondary)",
            backgroundColor: "transparent",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
            whiteSpace: "nowrap",
          }}
          title="重新拉取群名/单聊名与类型"
        >
          刷新会话信息
        </button>
        {notice && (
          <span style={{ fontSize: "11px", color: "var(--text-muted)", whiteSpace: "nowrap" }}>
            {notice}
          </span>
        )}
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
        {loading && events.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            加载中…
          </div>
        ) : events.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            该会话还没有消息
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "14px" }}>
            {events.map((event, index) => {
              const streaming = streamingByMessage[event.message_id];
              const live =
                streaming != null &&
                event.reply_status == null &&
                Date.now() - streaming.updatedAt < LIVE_TIMEOUT_MS;
              const status = statusText(event, live);
              // 生成中：思考块用增量文本；生成完：用落库的整段。
              const thinking = live ? streaming.thinking : event.reasoning;
              const answer = live ? streaming.answer : event.reply_text;
              const showThinking = thinking != null && thinking !== "";

              return (
                <div key={`${event.message_id}-${index}`}>
                  <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginBottom: "4px" }}>
                    {event.sender || "(未知发送人)"} ·{" "}
                    {new Date(event.received_at).toLocaleString()} · {event.listen_kind}
                    {event.malformed && (
                      <span style={{ color: "var(--danger)", marginLeft: "6px" }}>畸形事件</span>
                    )}
                  </div>

                  <div style={{ ...bubbleBase, backgroundColor: "var(--bg-elevated)", border: "1px solid var(--border)", color: "var(--text-primary)" }}>
                    {event.content || "(空正文)"}
                  </div>

                  {showThinking && (
                    <ThinkingBlock text={thinking} live={live && !answer} />
                  )}

                  {live && !answer && (
                    <div
                      style={{
                        fontSize: "11px",
                        color: "var(--warn)",
                        marginTop: "4px",
                      }}
                    >
                      {showThinking ? "正在组织回复…" : "思考中…"}
                    </div>
                  )}

                  {answer != null && answer !== "" && (
                    <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", marginTop: "6px" }}>
                      <div
                        style={{
                          ...bubbleBase,
                          backgroundColor: "var(--accent)",
                          color: "var(--accent-contrast)",
                        }}
                      >
                        {live ? <Typewriter text={answer} /> : answer}
                      </div>
                    </div>
                  )}

                  <div style={{ fontSize: "11px", color: status.color, marginTop: "4px" }}>
                    {event.reply_status === "sent" && event.received_at
                      ? `${status.text} ${new Date(event.received_at).toLocaleTimeString()}`
                      : status.text}
                  </div>
                </div>
              );
            })}
            <div ref={bottomRef} />
          </div>
        )}
      </div>
    </div>
  );
}