import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface EventRow {
  project_id: string;
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
}

interface ProjectOption {
  id: string;
  name: string;
}

/** 会话筛选用的候选项：名字 + 群聊/单聊，便于在下拉里认出是哪条会话。 */
interface ConversationOption {
  conversation_id: string;
  name: string;
  kind: string;
}

interface EventLogProps {
  refreshToken: number;
  projects: ProjectOption[];
}

const PAGE_SIZE = 200;

const STATUS: Record<string, { label: string; color: string }> = {
  sent: { label: "已回复", color: "var(--success)" },
  failed: { label: "回复失败", color: "var(--danger)" },
  skipped: { label: "已跳过", color: "var(--text-muted)" },
};

/** 查询跨度上限 31 天，与后端校验保持一致。 */
function spanExceedsLimit(since: string, until: string) {
  if (!since || !until) return false;
  const from = new Date(`${since}T00:00:00`);
  const to = new Date(`${until}T00:00:00`);
  if (Number.isNaN(from.getTime()) || Number.isNaN(to.getTime())) return false;
  return (to.getTime() - from.getTime()) / 86_400_000 > 31;
}

const control = { padding: "4px 8px", fontSize: "12px" } as const;

const smallButton = {
  padding: "4px 10px",
  fontSize: "12px",
  backgroundColor: "transparent",
  color: "var(--text-secondary)",
  border: "1px solid var(--border)",
  borderRadius: "4px",
  cursor: "pointer",
} as const;

/**
 * 事件与回复合成一个视图：收到的每条消息就是一行，
 * 回复/跳过/失败都是这一行上的结果 —— 原来分成两个页签，看同一条消息要来回切。
 */
export default function EventLog({ refreshToken, projects }: EventLogProps) {
  const [events, setEvents] = useState<EventRow[]>([]);
  const [projectId, setProjectId] = useState("");
  const [conversationId, setConversationId] = useState("");
  const [conversations, setConversations] = useState<ConversationOption[]>([]);
  const [keyword, setKeyword] = useState("");
  const [kind, setKind] = useState("");
  const [malformedOnly, setMalformedOnly] = useState(false);
  const [failedOnly, setFailedOnly] = useState(false);
  const [sinceDate, setSinceDate] = useState("");
  const [untilDate, setUntilDate] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [rangeError, setRangeError] = useState<string | null>(null);
  const offsetRef = useRef(0);

  const fetchPage = useCallback(
    async (offset: number) => {
      return invoke<EventRow[]>("list_events", {
        limit: PAGE_SIZE,
        offset,
        projectId: projectId || null,
        conversationId: conversationId || null,
        keyword: keyword || null,
        malformedOnly,
        failedOnly,
        sinceDate: sinceDate || null,
        untilDate: untilDate || null,
      });
    },
    [projectId, conversationId, keyword, malformedOnly, failedOnly, sinceDate, untilDate],
  );

  const reload = useCallback(async () => {
    if (spanExceedsLimit(sinceDate, untilDate)) {
      setRangeError("查询跨度不能超过 31 天");
      setEvents([]);
      setHasMore(false);
      return;
    }
    setRangeError(null);
    try {
      const result = await fetchPage(0);
      setEvents(result);
      offsetRef.current = result.length;
      setHasMore(result.length === PAGE_SIZE);
    } catch (e) {
      setRangeError(String(e));
    }
  }, [fetchPage, sinceDate, untilDate]);

  const loadMore = async () => {
    try {
      const result = await fetchPage(offsetRef.current);
      setEvents((current) => [...current, ...result]);
      offsetRef.current += result.length;
      setHasMore(result.length === PAGE_SIZE);
    } catch (e) {
      setRangeError(String(e));
    }
  };

  useEffect(() => {
    reload();
  }, [reload, refreshToken]);

  // 会话候选项跟着项目走：选了项目就列它的会话，没选就列全部会话。
  // 项目变了要把已选会话清掉，否则会拿着别的项目的会话 id 去筛。
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const rows = await invoke<ConversationOption[]>("list_conversations", {
          projectId: projectId || null,
          unassigned: false,
        });
        if (!cancelled) setConversations(rows);
      } catch (e) {
        console.error("Failed to load conversations:", e);
        if (!cancelled) setConversations([]);
      }
    };
    load();
    setConversationId("");
    const timer = setInterval(load, 5000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [projectId, refreshToken]);

  useEffect(() => {
    if (!autoRefresh) return;
    const timer = setInterval(reload, 2000);
    return () => clearInterval(timer);
  }, [autoRefresh, reload]);

  // 来源筛选放在前端做：一页只有 200 条，再回一次后端反而更慢。
  const shown = kind ? events.filter((e) => e.listen_kind === kind) : events;
  const projectName = (id: string) =>
    projects.find((project) => project.id === id)?.name ?? (id || "未归类");

  const handleExport = () => {
    const blob = new Blob([shown.map((event) => JSON.stringify(event)).join("\n")], {
      type: "application/x-ndjson",
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `agentmux-events-${new Date().toISOString().slice(0, 10)}.ndjson`;
    link.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div
        style={{
          padding: "10px 16px",
          borderBottom: "1px solid var(--border)",
          backgroundColor: "var(--bg-sidebar)",
          display: "flex",
          gap: "8px",
          alignItems: "center",
          flexWrap: "wrap",
        }}
      >
        <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-primary)" }}>
          事件与回复
        </span>
        <select
          value={projectId}
          onChange={(e) => setProjectId(e.target.value)}
          style={control}
        >
          <option value="">全部项目</option>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
        <input
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
          placeholder="搜索正文"
          style={{ ...control, width: "140px" }}
        />
        <select value={kind} onChange={(e) => setKind(e.target.value)} style={control}>
          <option value="">全部来源</option>
          <option value="at-me">@我</option>
          <option value="all-direct">单聊</option>
        </select>
        <select
          value={conversationId}
          onChange={(e) => setConversationId(e.target.value)}
          title="只这一条会话的事件"
          style={{ ...control, maxWidth: "220px" }}
        >
          <option value="">全部会话{conversations.length > 0 ? `（${conversations.length}）` : ""}</option>
          {conversations.map((conversation) => (
            <option key={conversation.conversation_id} value={conversation.conversation_id}>
              {conversation.kind === "group" ? "群 " : conversation.kind === "direct" ? "单聊 " : ""}
              {conversation.name || conversation.conversation_id}
            </option>
          ))}
        </select>
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          自
          <input
            type="date"
            value={sinceDate}
            onChange={(e) => setSinceDate(e.target.value)}
            style={{ margin: "0 4px", padding: "3px 6px", fontSize: "12px" }}
          />
        </label>
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          至
          <input
            type="date"
            value={untilDate}
            onChange={(e) => setUntilDate(e.target.value)}
            style={{ margin: "0 4px", padding: "3px 6px", fontSize: "12px" }}
          />
        </label>
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          <input
            type="checkbox"
            checked={failedOnly}
            onChange={(e) => setFailedOnly(e.target.checked)}
            style={{ marginRight: "4px", accentColor: "var(--accent)" }}
          />
          只看失败
        </label>
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          <input
            type="checkbox"
            checked={malformedOnly}
            onChange={(e) => setMalformedOnly(e.target.checked)}
            style={{ marginRight: "4px", accentColor: "var(--accent)" }}
          />
          仅畸形
        </label>
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          <input
            type="checkbox"
            checked={autoRefresh}
            onChange={(e) => setAutoRefresh(e.target.checked)}
            style={{ marginRight: "4px", accentColor: "var(--accent)" }}
          />
          自动刷新
        </label>
        <div style={{ marginLeft: "auto", display: "flex", gap: "8px" }}>
          <button onClick={reload} style={smallButton}>
            刷新
          </button>
          <button
            onClick={handleExport}
            style={{
              padding: "4px 10px",
              fontSize: "12px",
              backgroundColor: "var(--accent)",
              color: "var(--accent-contrast)",
              border: "none",
              borderRadius: "4px",
              cursor: "pointer",
            }}
          >
            导出 ndjson
          </button>
        </div>
      </div>

      {rangeError && (
        <div
          style={{
            padding: "6px 16px",
            backgroundColor: "var(--danger-strong)",
            color: "#fff",
            fontSize: "12px",
          }}
        >
          {rangeError}
        </div>
      )}

      <div style={{ flex: 1, overflow: "auto", padding: "12px 16px" }}>
        {shown.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            暂无事件。启动监听后，收到的消息与回复结果都会出现在这里。
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
            {shown.map((event, index) => {
              const key = `${event.message_id}-${index}`;
              const isOpen = expanded === key;
              const status = event.reply_status
                ? (STATUS[event.reply_status] ?? {
                    label: event.reply_status,
                    color: "var(--text-muted)",
                  })
                : null;
              return (
                <div
                  key={key}
                  style={{
                    border: "1px solid var(--border)",
                    borderRadius: "6px",
                    backgroundColor: "var(--bg-elevated)",
                    overflow: "hidden",
                  }}
                >
                  <div style={{ padding: "8px 12px" }}>
                    <div
                      style={{
                        display: "flex",
                        gap: "8px",
                        alignItems: "center",
                        fontSize: "11px",
                        color: "var(--text-secondary)",
                        flexWrap: "wrap",
                      }}
                    >
                      {status ? (
                        <span style={{ color: status.color }}>{status.label}</span>
                      ) : (
                        <span style={{ color: "var(--text-muted)" }}>未处理</span>
                      )}
                      <span style={{ color: "var(--accent)" }}>{projectName(event.project_id)}</span>
                      <span style={{ color: "var(--text-muted)" }}>{event.listen_kind}</span>
                      <span>{event.sender || "(未知发送人)"}</span>
                      <span>{new Date(event.received_at).toLocaleString()}</span>
                      <span
                        style={{
                          fontFamily: "ui-monospace, Consolas, monospace",
                          color: "var(--text-muted)",
                        }}
                      >
                        {event.conversation_id || "(无会话)"}
                      </span>
                      {event.malformed && <span style={{ color: "var(--danger)" }}>畸形</span>}
                      <button
                        onClick={() => setExpanded(isOpen ? null : key)}
                        style={{
                          marginLeft: "auto",
                          padding: "2px 6px",
                          fontSize: "11px",
                          backgroundColor: "transparent",
                          color: "var(--text-secondary)",
                          border: "1px solid var(--border)",
                          borderRadius: "3px",
                          cursor: "pointer",
                        }}
                      >
                        {isOpen ? "收起" : "查看原文"}
                      </button>
                    </div>

                    <div
                      style={{
                        marginTop: "6px",
                        fontSize: "13px",
                        color: "var(--text-primary)",
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                      }}
                    >
                      <span style={{ color: "var(--text-muted)", fontSize: "12px" }}>来信：</span>
                      {event.content || "(空正文)"}
                    </div>

                    {event.reply_status === "sent" && event.reply_text && (
                      <div
                        style={{
                          marginTop: "4px",
                          fontSize: "12px",
                          color: "var(--text-secondary)",
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                        }}
                      >
                        <span style={{ color: "var(--text-muted)" }}>回复：</span>
                        {event.reply_text}
                      </div>
                    )}

                    {/* 跳过的原因必须显示，否则只看到「已跳过」查不出为什么没回复 */}
                    {event.reply_status === "skipped" && event.reply_text && (
                      <div
                        style={{
                          marginTop: "4px",
                          fontSize: "11px",
                          color: "var(--text-muted)",
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                        }}
                      >
                        <span>跳过原因：</span>
                        {event.reply_text}
                      </div>
                    )}

                    {event.reply_status === "failed" && (
                      <div
                        style={{
                          marginTop: "6px",
                          padding: "8px",
                          backgroundColor: "var(--bg-app)",
                          border: "1px solid var(--danger)",
                          borderRadius: "4px",
                          fontSize: "11px",
                          color: "var(--danger)",
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                          fontFamily: "ui-monospace, Consolas, monospace",
                        }}
                      >
                        {event.reply_text ?? "(未记录失败原文)"}
                      </div>
                    )}
                  </div>
                  {isOpen && (
                    <pre
                      style={{
                        margin: 0,
                        padding: "10px 12px",
                        borderTop: "1px solid var(--border)",
                        backgroundColor: "var(--bg-app)",
                        color: "var(--text-secondary)",
                        fontSize: "11px",
                        overflow: "auto",
                        maxHeight: "220px",
                      }}
                    >
                      {JSON.stringify(event, null, 2)}
                    </pre>
                  )}
                </div>
              );
            })}

            {hasMore && (
              <button
                onClick={loadMore}
                style={{
                  padding: "8px",
                  fontSize: "12px",
                  backgroundColor: "transparent",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                加载更多（已显示 {shown.length} 条）
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
