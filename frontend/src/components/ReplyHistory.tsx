import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
}

const STATUS: Record<string, { label: string; color: string }> = {
  sent: { label: "已发送", color: "var(--success)" },
  failed: { label: "发送失败", color: "var(--danger)" },
  skipped: { label: "已跳过", color: "var(--text-muted)" },
};

export default function ReplyHistory() {
  const [events, setEvents] = useState<EventRow[]>([]);
  const [failedOnly, setFailedOnly] = useState(false);
  const [keyword, setKeyword] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const result = await invoke<EventRow[]>("list_events", {
        limit: 200,
        keyword: keyword || null,
        failedOnly,
      });
      // 回复历史只关心已判定过的消息。
      setEvents(result.filter((event) => event.reply_status !== null));
    } catch (e) {
      console.error("Failed to load reply history:", e);
    }
  }, [failedOnly, keyword]);

  useEffect(() => {
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  }, [load]);

  /** A5.3.3 / D-50：核对发送身份后一键回填到设置。 */
  const adoptIdentity = async (openDingTalkId: string) => {
    if (!openDingTalkId) return;
    if (!confirm(`把 ${openDingTalkId} 设为「自身身份」？此后将跳过该身份发送的消息。`)) return;
    try {
      const config = await invoke<Record<string, unknown>>("get_config");
      await invoke("set_config", {
        config: { ...config, self_open_dingtalk_id: openDingTalkId },
      });
      await invoke("apply_settings");
      setNotice("已回填自身身份并在运行期生效");
    } catch (e) {
      setNotice("回填失败：" + String(e));
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div
        style={{
          padding: "10px 16px",
          borderBottom: "1px solid var(--border)",
          backgroundColor: "var(--bg-sidebar)",
          display: "flex",
          gap: "10px",
          alignItems: "center",
          flexWrap: "wrap",
        }}
      >
        <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-primary)" }}>
          回复历史
        </span>
        <input
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
          placeholder="搜索正文"
          style={{ padding: "4px 8px", fontSize: "12px", width: "150px" }}
        />
        <label style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
          <input
            type="checkbox"
            checked={failedOnly}
            onChange={(e) => setFailedOnly(e.target.checked)}
            style={{ marginRight: "4px", accentColor: "var(--accent)" }}
          />
          只看失败
        </label>
        <span style={{ marginLeft: "auto", fontSize: "11px", color: "var(--text-muted)" }}>
          共 {events.length} 条
        </span>
      </div>

      {notice && (
        <div
          style={{
            padding: "6px 16px",
            backgroundColor: "var(--accent)",
            color: "var(--accent-contrast)",
            fontSize: "12px",
          }}
        >
          {notice}
        </div>
      )}

      <div style={{ flex: 1, overflow: "auto", padding: "12px 16px" }}>
        {events.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            暂无回复记录。启用自动回复后，这里会列出每条消息的处理结果。
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
            {events.map((event, index) => {
              const status = STATUS[event.reply_status ?? ""] ?? {
                label: event.reply_status ?? "未知",
                color: "var(--text-muted)",
              };
              const key = `${event.message_id}-${index}`;
              const isOpen = expanded === key;
              return (
                <div
                  key={key}
                  style={{
                    border: "1px solid var(--border)",
                    borderRadius: "6px",
                    backgroundColor: "var(--bg-elevated)",
                    padding: "10px 12px",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      gap: "10px",
                      alignItems: "center",
                      fontSize: "11px",
                      color: "var(--text-secondary)",
                      flexWrap: "wrap",
                    }}
                  >
                    <span style={{ color: status.color }}>{status.label}</span>
                    <span>{event.sender || "(未知发送人)"}</span>
                    <span>{new Date(event.received_at).toLocaleString()}</span>
                    <span style={{ color: "var(--text-muted)" }}>{event.listen_kind}</span>
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
                      {isOpen ? "收起" : "明细"}
                    </button>
                  </div>

                  <div
                    style={{
                      marginTop: "6px",
                      fontSize: "12px",
                      color: "var(--text-primary)",
                      whiteSpace: "pre-wrap",
                      wordBreak: "break-word",
                    }}
                  >
                    <span style={{ color: "var(--text-muted)" }}>来信：</span>
                    {event.content || "(空正文)"}
                  </div>

                  {event.reply_text && event.reply_status === "sent" && (
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

                  {/* A5.3.2：失败原因保留原文，不做美化 */}
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

                  {isOpen && (
                    <div
                      style={{
                        marginTop: "8px",
                        paddingTop: "8px",
                        borderTop: "1px solid var(--border)",
                        fontSize: "11px",
                        color: "var(--text-muted)",
                        display: "flex",
                        gap: "12px",
                        flexWrap: "wrap",
                        alignItems: "center",
                      }}
                    >
                      <span style={{ fontFamily: "ui-monospace, Consolas, monospace" }}>
                        sender_id: {event.sender_open_dingtalk_id || "(空)"}
                      </span>
                      <span style={{ fontFamily: "ui-monospace, Consolas, monospace" }}>
                        message_id: {event.message_id || "(空)"}
                      </span>
                      <span style={{ fontFamily: "ui-monospace, Consolas, monospace" }}>
                        conversation: {event.conversation_id}
                      </span>
                      {event.sender_open_dingtalk_id && (
                        <button
                          onClick={() => adoptIdentity(event.sender_open_dingtalk_id)}
                          style={{
                            padding: "2px 8px",
                            fontSize: "11px",
                            backgroundColor: "var(--accent)",
                            color: "var(--accent-contrast)",
                            border: "none",
                            borderRadius: "3px",
                            cursor: "pointer",
                          }}
                        >
                          采用该身份
                        </button>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
