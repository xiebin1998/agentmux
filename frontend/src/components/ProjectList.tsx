import { useState } from "react";

interface Project {
  id: string;
  name: string;
  work_dir: string;
  agent_cli_path: string;
  dingtalk_cli_path: string;
  im_platform: string;
  agent_platform: string;
  reply_enabled: boolean;
  reply_timeout_ms: number;
  reply_max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
  created_at: string;
  updated_at: string;
}

interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
}

type ListenKind = "at-me" | "all-direct";

interface ListenerStatus {
  id: string;
  project_id: string;
  kind: ListenKind;
  state: string;
  ready: boolean;
  subscribe_id: string | null;
  last_error: string | null;
}

const KINDS: { kind: ListenKind; label: string }[] = [
  { kind: "at-me", label: "@我" },
  { kind: "all-direct", label: "单聊" },
];

function statusColor(status: ListenerStatus | undefined) {
  if (!status) return "var(--text-muted)";
  if (status.ready) return "var(--success)";
  if (status.state === "abandoned") return "var(--danger)";
  if (status.state === "starting" || status.state === "backing_off") return "var(--warn)";
  return "var(--text-muted)";
}

function statusText(status: ListenerStatus | undefined) {
  if (!status) return "未启动";
  if (status.ready) return "监听中";
  switch (status.state) {
    case "starting":
      return "启动中";
    case "backing_off":
      return "重试中";
    case "abandoned":
      return "已放弃";
    case "stopped":
      return "已停止";
    default:
      return status.state;
  }
}

interface ProjectListProps {
  projects: Project[];
  /** project_id → 该项目的会话 */
  sessionsByProject: Record<string, Session[]>;
  /** 没有项目归属的历史会话（升级前的数据） */
  unassignedSessions: Session[];
  /** project_id → kind → 监听状态 */
  statusesByProject: Record<string, Partial<Record<ListenKind, ListenerStatus>>>;
  selectedSession: Session | null;
  busyKey: string | null;
  onSelectSession: (session: Session, project: Project | null) => void;
  onAssignConversation: (conversationId: string, projectId: string) => void;
  onEditProject: (project: Project) => void;
  onDeleteProject: (id: string) => void;
  onToggleListener: (project: Project, kind: ListenKind, active: boolean) => void;
}

const iconButton = {
  padding: "2px 6px",
  backgroundColor: "transparent",
  color: "var(--text-secondary)",
  border: "none",
  borderRadius: "3px",
  cursor: "pointer",
  fontSize: "11px",
} as const;

export default function ProjectList({
  projects,
  sessionsByProject,
  unassignedSessions,
  statusesByProject,
  selectedSession,
  busyKey,
  onSelectSession,
  onAssignConversation,
  onEditProject,
  onDeleteProject,
  onToggleListener,
}: ProjectListProps) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  /** 每个未归类会话当前选中的目标项目 */
  const [assignTarget, setAssignTarget] = useState<Record<string, string>>({});

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
        项目
      </div>

      {projects.length === 0 ? (
        <div style={{ padding: "12px", color: "var(--text-muted)", fontSize: "13px" }}>
          暂无项目，点击右上角「创建项目」
        </div>
      ) : (
        projects.map((project) => {
          const sessions = sessionsByProject[project.id] ?? [];
          const statuses = statusesByProject[project.id] ?? {};
          const isCollapsed = collapsed[project.id] ?? false;

          return (
            <div key={project.id} style={{ borderBottom: "1px solid var(--border)" }}>
              {/* 项目行：名称 + 本项目自己的监听开关 */}
              <div style={{ padding: "8px 12px" }}>
                <div style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                  <button
                    onClick={() =>
                      setCollapsed((current) => ({
                        ...current,
                        [project.id]: !isCollapsed,
                      }))
                    }
                    style={{ ...iconButton, color: "var(--text-secondary)" }}
                    title={isCollapsed ? "展开会话" : "收起会话"}
                  >
                    {isCollapsed ? "▶" : "▼"}
                  </button>
                  <span
                    style={{ flex: 1, color: "var(--text-primary)", fontSize: "13px" }}
                    title={project.work_dir}
                  >
                    {project.name}
                  </span>
                  <button onClick={() => onEditProject(project)} style={iconButton}>
                    编辑
                  </button>
                  <button
                    onClick={() => onDeleteProject(project.id)}
                    style={{ ...iconButton, color: "var(--danger)" }}
                  >
                    删除
                  </button>
                </div>

                {/* 每个项目独立控制 @我 / 单聊 */}
                <div style={{ display: "flex", gap: "6px", marginTop: "6px", flexWrap: "wrap" }}>
                  {KINDS.map(({ kind, label }) => {
                    const status = statuses[kind];
                    const active = Boolean(status?.ready);
                    const key = `${project.id}:${kind}`;
                    const busy = busyKey === key;
                    return (
                      <div
                        key={kind}
                        style={{
                          display: "flex",
                          alignItems: "center",
                          gap: "4px",
                          padding: "2px 6px",
                          border: "1px solid var(--border)",
                          borderRadius: "4px",
                        }}
                        title={status?.last_error ?? undefined}
                      >
                        <span style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
                          {label}
                        </span>
                        <span style={{ fontSize: "11px", color: statusColor(status) }}>
                          {statusText(status)}
                        </span>
                        <button
                          onClick={() => onToggleListener(project, kind, active)}
                          disabled={busy}
                          style={{
                            padding: "1px 6px",
                            fontSize: "10px",
                            border: "none",
                            borderRadius: "3px",
                            cursor: busy ? "not-allowed" : "pointer",
                            backgroundColor: active ? "var(--danger-strong)" : "var(--accent)",
                            color: "var(--accent-contrast)",
                          }}
                        >
                          {busy ? "…" : active ? "停止" : "启动"}
                        </button>
                      </div>
                    );
                  })}
                </div>

                <div style={{ fontSize: "10px", color: "var(--text-muted)", marginTop: "4px" }}>
                  工作目录：{project.work_dir}
                </div>
              </div>

              {/* 该项目下的会话 */}
              {!isCollapsed && (
                <div style={{ paddingBottom: "6px" }}>
                  {sessions.length === 0 ? (
                    <div
                      style={{
                        padding: "4px 12px 6px 28px",
                        fontSize: "11px",
                        color: "var(--text-muted)",
                      }}
                    >
                      暂无会话（收到消息后自动出现）
                    </div>
                  ) : (
                    sessions.map((session) => {
                      const selected = selectedSession?.id === session.conversation_id;
                      return (
                        <div
                          key={session.conversation_id}
                          onClick={() => onSelectSession(session, project)}
                          style={{
                            padding: "5px 12px 5px 28px",
                            cursor: "pointer",
                            backgroundColor: selected ? "var(--bg-active)" : "transparent",
                          }}
                        >
                          <div style={{ fontSize: "12px", color: "var(--text-primary)" }}>
                            {session.name}
                          </div>
                          <div
                            style={{
                              fontSize: "10px",
                              color: "var(--text-muted)",
                              fontFamily: "ui-monospace, Consolas, monospace",
                              wordBreak: "break-all",
                            }}
                          >
                            {session.conversation_id}
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              )}
            </div>
          );
        })
      )}
      {/* 升级前的历史会话：没有项目归属，但必须可见、可归入 */}
      {unassignedSessions.length > 0 && (
        <div style={{ borderBottom: "1px solid var(--border)" }}>
          <div style={{ padding: "8px 12px 2px", fontSize: "12px", color: "var(--warn)" }}>
            未归类（历史会话）· {unassignedSessions.length}
          </div>
          <div
            style={{
              padding: "0 12px 6px",
              fontSize: "10px",
              color: "var(--text-muted)",
              lineHeight: 1.5,
            }}
          >
            这些是升级前收到的消息，还没有归入任何项目。选一个项目点「归入」即可。
          </div>
          {unassignedSessions.map((session) => {
            const target = assignTarget[session.conversation_id] ?? "";
            const selected = selectedSession?.id === session.conversation_id;
            return (
              <div
                key={session.conversation_id}
                style={{
                  padding: "6px 12px 8px",
                  backgroundColor: selected ? "var(--bg-active)" : "transparent",
                }}
              >
                <div
                  onClick={() => onSelectSession(session, null)}
                  style={{ cursor: "pointer" }}
                >
                  <div style={{ fontSize: "12px", color: "var(--text-primary)" }}>
                    {session.name}
                  </div>
                  <div
                    style={{
                      fontSize: "10px",
                      color: "var(--text-muted)",
                      fontFamily: "ui-monospace, Consolas, monospace",
                      wordBreak: "break-all",
                    }}
                  >
                    {session.conversation_id}
                  </div>
                </div>
                <div style={{ display: "flex", gap: "6px", marginTop: "4px" }}>
                  <select
                    value={target}
                    onChange={(e) =>
                      setAssignTarget((current) => ({
                        ...current,
                        [session.conversation_id]: e.target.value,
                      }))
                    }
                    style={{
                      flex: 1,
                      minWidth: 0,
                      padding: "2px 4px",
                      fontSize: "11px",
                      backgroundColor: "var(--bg-input)",
                      color: "var(--text-primary)",
                      border: "1px solid var(--border)",
                      borderRadius: "3px",
                    }}
                  >
                    <option value="">选择项目…</option>
                    {projects.map((project) => (
                      <option key={project.id} value={project.id}>
                        {project.name}
                      </option>
                    ))}
                  </select>
                  <button
                    disabled={!target}
                    onClick={() => onAssignConversation(session.conversation_id, target)}
                    style={{
                      padding: "2px 8px",
                      fontSize: "11px",
                      backgroundColor: target ? "var(--accent)" : "var(--text-muted)",
                      color: "var(--accent-contrast)",
                      border: "none",
                      borderRadius: "3px",
                      cursor: target ? "pointer" : "not-allowed",
                    }}
                  >
                    归入
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
