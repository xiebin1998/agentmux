import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import Typewriter from "./Typewriter";
import CompressionSection from "./CompressionSection";
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
  /** 这次用过的工具（每行一条）；没用工具或还没回过就是 null。 */
  tools: string | null;
  /** 这批回复挂在哪条消息上；null = 单条回复或老记录，直接在自身展开。 */
  reply_anchor: string | null;
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

/** conversation_session 命令的返回：这个会话在 Agent 侧的建档情况。 */
interface AgentSessionInfo {
  conversation_id: string;
  agent_session_id: string;
  agent_cwd: string;
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
  onDeleteSession: () => void;
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

/** 「⋯」菜单里的一行操作。 */
const menuAction: CSSProperties = {
  padding: "5px 10px",
  fontSize: "12px",
  textAlign: "left",
  backgroundColor: "transparent",
  color: "var(--text-secondary)",
  border: "1px solid var(--border)",
  borderRadius: "4px",
  cursor: "pointer",
};

/** 会话 id / 建档目录这类要能整段选中复制的值。 */
const monoStyle: CSSProperties = {
  marginTop: "2px",
  fontFamily: "ui-monospace, Consolas, monospace",
  color: "var(--text-primary)",
  wordBreak: "break-all",
  userSelect: "all",
};

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

export default function MessageView({
  session,
  streamingByMessage,
  onRefreshMeta,
  onDeleteSession,
}: MessageViewProps) {
  const [events, setEvents] = useState<EventRow[]>([]);
  const [details, setDetails] = useState<ConversationDetails | null>(null);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [agentSession, setAgentSession] = useState<AgentSessionInfo | null>(null);
  /** 作废会话后要重取建档信息。 */
  const [metaVersion, setMetaVersion] = useState(0);
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

  // Agent 侧的建档情况只在「⋯」里看，不轮询：它只在回复或作废时变。
  useEffect(() => {
    let cancelled = false;
    invoke<AgentSessionInfo | null>("conversation_session", {
      projectId: session.project_id,
      conversationId: session.conversation_id,
    })
      .then((result) => {
        if (!cancelled) setAgentSession(result);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [session.project_id, session.conversation_id, metaVersion]);

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

        <div style={{ position: "relative" }}>
          <button
            onClick={() => setMenuOpen((open) => !open)}
            style={{
              padding: "2px 8px",
              fontSize: "13px",
              lineHeight: 1,
              color: "var(--text-secondary)",
              backgroundColor: menuOpen ? "var(--bg-active)" : "transparent",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              cursor: "pointer",
            }}
            title="更多会话操作"
          >
            ⋯
          </button>

          {menuOpen && (
            <div
              style={{
                position: "absolute",
                top: "calc(100% + 6px)",
                right: 0,
                zIndex: 20,
                width: "320px",
                maxHeight: "70vh",
                overflow: "auto",
                padding: "12px",
                backgroundColor: "var(--bg-sidebar)",
                border: "1px solid var(--border)",
                borderRadius: "6px",
                boxShadow: "0 8px 24px rgba(0,0,0,.28)",
              }}
            >
              <div
                style={{
                  display: "flex",
                  flexDirection: "column",
                  gap: "4px",
                  marginBottom: "10px",
                  fontSize: "12px",
                }}
              >
                <div style={{ display: "flex", justifyContent: "space-between" }}>
                  <span style={{ color: "var(--text-secondary)" }}>Agent 会话</span>
                  <span style={{ color: agentSession ? "var(--success)" : "var(--text-muted)" }}>
                    {agentSession ? "已建档" : "未建档（下一条消息会新建）"}
                  </span>
                </div>
                {agentSession && (
                  <>
                    <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                      会话 id
                      <div style={monoStyle}>{agentSession.agent_session_id}</div>
                    </div>
                    <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                      建档工作目录
                      <div style={monoStyle}>{agentSession.agent_cwd}</div>
                    </div>
                  </>
                )}
              </div>

              <div
                style={{
                  display: "flex",
                  flexDirection: "column",
                  gap: "6px",
                  paddingBottom: "10px",
                  borderBottom: "1px solid var(--border)",
                }}
              >
                <button
                  onClick={async () => {
                    try {
                      setNotice(await onRefreshMeta());
                    } catch (e) {
                      setNotice(`同步失败：${e}`);
                    }
                  }}
                  style={menuAction}
                >
                  刷新会话信息（群名/单聊名）
                </button>
                <button
                  onClick={async () => {
                    if (
                      !confirm(
                        "作废该会话的 Agent 会话记录？下一条消息会重新建档（历史事件不受影响）。",
                      )
                    ) {
                      return;
                    }
                    try {
                      await invoke("reset_conversation", {
                        projectId: session.project_id,
                        conversationId: session.conversation_id,
                      });
                      setNotice("已作废，下一条消息将重建会话");
                      setMetaVersion((version) => version + 1);
                    } catch (e) {
                      setNotice(`作废失败：${e}`);
                    }
                  }}
                  style={menuAction}
                >
                  作废并重建 Agent 会话
                </button>
                <button
                  onClick={() => {
                    setMenuOpen(false);
                    onDeleteSession();
                  }}
                  style={{ ...menuAction, color: "var(--danger)" }}
                >
                  删除会话
                </button>
              </div>

              <CompressionSection
                projectId={session.project_id}
                conversationId={session.conversation_id}
              />
            </div>
          )}
        </div>

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
              const mergedIntoAnchor =
                event.reply_anchor != null && event.reply_anchor !== event.message_id;
              // 合并回复只在锚点那条上展开成一轮助手输出；其余批内消息只标注一句，
              // 否则同一段思考/工具/回复会在对话流里重复出现。
              const status = mergedIntoAnchor
                ? { text: "已合进下面那条的回复", color: "var(--text-muted)" }
                : statusText(event, live);
              // 生成中：思考块用增量文本；生成完：用落库的整段。
              const thinking = mergedIntoAnchor ? null : live ? streaming.thinking : event.reasoning;
              const answer = mergedIntoAnchor ? null : live ? streaming.answer : event.reply_text;
              const showThinking = thinking != null && thinking !== "";
              const tools = mergedIntoAnchor
                ? []
                : live && streaming.tools.length > 0
                  ? streaming.tools
                  : (event.tools ?? "").split("\n").filter((line) => line.trim() !== "");

              return (
                <div key={`${event.message_id}-${index}`}>
                  {/* 对方：靠左。放在 flex 列里才会**缩到内容宽** ——
                      直接给块级 div 加 maxWidth，短消息也会被撑成 78% 宽。 */}
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "flex-start",
                      gap: "4px",
                    }}
                  >
                    <div style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
                      {event.sender || "(未知发送人)"} ·{" "}
                      {new Date(event.received_at).toLocaleString()} · {event.listen_kind}
                      {event.malformed && (
                        <span style={{ color: "var(--danger)", marginLeft: "6px" }}>畸形事件</span>
                      )}
                    </div>
                    <div
                      style={{
                        ...bubbleBase,
                        backgroundColor: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        color: "var(--text-primary)",
                      }}
                    >
                      {event.content || "(空正文)"}
                    </div>
                  </div>

                  {/* 助手侧：思考 → 工具 → 回复 → 状态，全在右边，与「回复」同一列。 */}
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "flex-end",
                      gap: "6px",
                      marginTop: "8px",
                    }}
                  >
                    {showThinking && <ThinkingBlock text={thinking} live={live && !answer} />}

                    {tools.length > 0 && (
                      <div
                        style={{
                          maxWidth: "78%",
                          display: "flex",
                          flexDirection: "column",
                          gap: "2px",
                          fontSize: "11px",
                          color: "var(--text-muted)",
                        }}
                      >
                        {tools.map((tool, toolIndex) => (
                          <div
                            key={`${tool}-${toolIndex}`}
                            style={{
                              fontFamily: "ui-monospace, Consolas, monospace",
                              wordBreak: "break-all",
                            }}
                          >
                            工具 · {tool}
                          </div>
                        ))}
                      </div>
                    )}

                    {live && !answer && (
                      <div style={{ fontSize: "11px", color: "var(--warn)" }}>
                        {showThinking || tools.length > 0 ? "正在组织回复…" : "思考中…"}
                      </div>
                    )}

                    {answer != null && answer !== "" && (
                      <div
                        style={{
                          ...bubbleBase,
                          backgroundColor: "var(--accent)",
                          color: "var(--accent-contrast)",
                        }}
                      >
                        {live ? <Typewriter text={answer} /> : answer}
                      </div>
                    )}

                    <div style={{ fontSize: "11px", color: status.color }}>
                      {event.reply_status === "sent" && event.received_at && !mergedIntoAnchor
                        ? `${status.text} ${new Date(event.received_at).toLocaleTimeString()}`
                        : status.text}
                    </div>
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