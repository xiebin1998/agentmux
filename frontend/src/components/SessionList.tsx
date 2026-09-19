interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
}

interface SessionListProps {
  sessions: Session[];
  selectedSession: Session | null;
  onSelectSession: (session: Session) => void;
}

export default function SessionList({
  sessions,
  selectedSession,
  onSelectSession,
}: SessionListProps) {
  return (
    <div style={{ flex: 1, overflow: "auto" }}>
      <div
        style={{
          padding: "8px 12px",
          fontSize: "11px",
          fontWeight: 600,
          color: "var(--text-secondary)",
          textTransform: "uppercase",
          letterSpacing: "0.5px",
        }}
      >
        会话
      </div>
      {sessions.length === 0 ? (
        <div style={{ padding: "12px", color: "var(--text-muted)", fontSize: "13px" }}>
          暂无会话
        </div>
      ) : (
        <div>
          {sessions.map((session) => {
            const selected = selectedSession?.id === session.id;
            return (
              <div
                key={session.id}
                onClick={() => onSelectSession(session)}
                style={{
                  padding: "8px 12px",
                  backgroundColor: selected ? "var(--bg-active)" : "transparent",
                  cursor: "pointer",
                }}
              >
                <div
                  style={{
                    color: "var(--text-primary)",
                    fontSize: "13px",
                    marginBottom: "4px",
                  }}
                >
                  {session.name}
                </div>
                <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                  {session.conversation_id}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
