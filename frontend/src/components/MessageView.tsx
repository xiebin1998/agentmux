import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
}

interface MessageViewProps {
  session: Session;
}

export default function MessageView({ session }: MessageViewProps) {
  const [events, setEvents] = useState<EventRow[]>([]);
  const [loading, setLoading] = useState(true);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      setLoading(true);
      try {
        const result = await invoke<EventRow[]>("list_events", {
          conversationId: session.conversation_id,
          limit: 200,
        });
        if (!cancelled) {
          setEvents([...result].reverse());
        }
      } catch (e) {
        console.error("Failed to load events:", e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    load();
    const timer = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [session.conversation_id]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div
        style={{
          padding: "12px 16px",
          borderBottom: "1px solid var(--border)",
          backgroundColor: "var(--bg-sidebar)",
        }}
      >
        <div style={{ color: "var(--text-primary)", fontSize: "14px", fontWeight: 600 }}>
          {session.name}
        </div>
        <div style={{ color: "var(--text-muted)", fontSize: "12px", marginTop: "4px" }}>
          {session.conversation_id}
        </div>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
        {loading && events.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            加载中…
          </div>
        ) : events.length === 0 ? (
          <div style={{ textAlign: "center", color: "var(--text-muted)", padding: "40px" }}>
            该会话还没有事件
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
            {events.map((event, index) => {
              const replied = event.reply_status === "sent";
              return (
                <div key={`${event.message_id}-${index}`}>
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "flex-start",
                    }}
                  >
                    <div
                      style={{
                        fontSize: "11px",
                        color: "var(--text-secondary)",
                        marginBottom: "4px",
                      }}
                    >
                      {event.sender || "(未知发送人)"} ·{" "}
                      {new Date(event.received_at).toLocaleString()} · {event.listen_kind}
                      {event.malformed && (
                        <span style={{ color: "var(--danger)", marginLeft: "6px" }}>畸形事件</span>
                      )}
                    </div>
                    <div
                      style={{
                        maxWidth: "78%",
                        padding: "8px 12px",
                        borderRadius: "8px",
                        backgroundColor: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        color: "var(--text-primary)",
                        fontSize: "13px",
                        lineHeight: 1.5,
                        wordBreak: "break-word",
                        whiteSpace: "pre-wrap",
                      }}
                    >
                      {event.content || "(空正文)"}
                    </div>
                  </div>

                  {event.reply_text && (
                    <div
                      style={{
                        display: "flex",
                        flexDirection: "column",
                        alignItems: "flex-end",
                        marginTop: "8px",
                      }}
                    >
                      <div
                        style={{
                          fontSize: "11px",
                          color: "var(--text-secondary)",
                          marginBottom: "4px",
                        }}
                      >
                        机器人回复 · {replied ? "已发送" : event.reply_status}
                      </div>
                      <div
                        style={{
                          maxWidth: "78%",
                          padding: "8px 12px",
                          borderRadius: "8px",
                          backgroundColor: "var(--accent)",
                          color: "var(--accent-contrast)",
                          fontSize: "13px",
                          lineHeight: 1.5,
                          wordBreak: "break-word",
                          whiteSpace: "pre-wrap",
                        }}
                      >
                        {event.reply_text}
                      </div>
                    </div>
                  )}
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
