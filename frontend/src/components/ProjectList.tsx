import { useState, type MouseEvent as ReactMouseEvent } from "react";
import type { Project, Session } from "../types";

/** 行操作菜单里的一项。 */
interface MenuItem {
  label: string;
  danger?: boolean;
  onClick: () => void;
}

/** 群聊 / 单聊标签：样式与文案都在这里，列表里两处复用。 */
function KindTag({ kind }: { kind: string }) {
  const label =
    kind === "group" ? "群聊" : kind === "direct" ? "单聊" : null;
  if (!label) return null;
  return (
    <span
      style={{
        marginRight: "5px",
        padding: "0 5px",
        borderRadius: "3px",
        fontSize: "10px",
        lineHeight: "15px",
        display: "inline-block",
        verticalAlign: "1px",
        color: kind === "group" ? "var(--accent)" : "var(--success)",
        border: `1px solid ${kind === "group" ? "var(--accent)" : "var(--success)"}`,
      }}
    >
      {label}
    </span>
  );
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
  onDeleteSession: (session: Session) => void;
  onEditProject: (project: Project) => void;
  onDeleteProject: (project: Project) => void;
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
  onDeleteSession,
  onEditProject,
  onDeleteProject,
  onToggleListener,
}: ProjectListProps) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  /** 每个未归类会话当前选中的目标项目 */
  const [assignTarget, setAssignTarget] = useState<Record<string, string>>({});
  /**
   * 行操作菜单：「⋯」点开的小菜单。
   *
   * 位置按按钮的实测坐标算、用 `fixed` 定位 —— 左列是滚动容器（overflow），
   * 用 absolute 会被裁掉；鼠标移上去才显示「⋯」本身，省得整列都是删除按钮。
   */
  const [menu, setMenu] = useState<{ top: number; left: number; items: MenuItem[] } | null>(null);

  const openRowMenu = (event: ReactMouseEvent<HTMLButtonElement>, items: MenuItem[]) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenu({ top: rect.bottom + 4, left: Math.max(8, rect.right - 132), items });
  };

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
                <div
                  className="amx-row"
                  style={{ display: "flex", alignItems: "center", gap: "6px" }}
                >
                  <button
                    onClick={() =>
                      setCollapsed((current) => ({
                        ...current,
                        [project.id]: !isCollapsed,
                      }))
                    }
                    style={{
                      ...iconButton,
                      color: "var(--text-secondary)",
                      display: "inline-flex",
                      alignItems: "center",
                      justifyContent: "center",
                      lineHeight: 0,
                    }}
                    title={isCollapsed ? "展开会话" : "收起会话"}
                  >
                    {/* 内联 SVG 而不是 ▶/▼ 文本字形：字形粗细随字体/缩放变化，很丑 */}
                    <svg
                      width="13"
                      height="13"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2.5"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      aria-hidden="true"
                      style={{
                        display: "block",
                        transform: isCollapsed ? "rotate(-90deg)" : "rotate(0deg)",
                        transition: "transform .15s ease",
                      }}
                    >
                      <polyline points="6 9 12 15 18 9" />
                    </svg>
                  </button>
                  <span
                    style={{ flex: 1, color: "var(--text-primary)", fontSize: "13px" }}
                    title={project.work_dir}
                  >
                    {project.name}
                  </span>
                  <button
                    className="amx-row-actions"
                    title="更多操作"
                    onClick={(e) =>
                      openRowMenu(e, [
                        { label: "编辑项目", onClick: () => onEditProject(project) },
                        {
                          label: "删除项目",
                          danger: true,
                          onClick: () => onDeleteProject(project),
                        },
                      ])
                    }
                    style={{ ...iconButton, lineHeight: 1, fontSize: "14px" }}
                  >
                    ⋯
                  </button>
                </div>

                {/* 每个项目独立控制 @我 / 单聊 */}
                <div style={{ display: "flex", gap: "6px", marginTop: "6px", flexWrap: "wrap" }}>
                  {KINDS.map(({ kind, label }) => {
                    const status = statuses[kind];
                    // 「有实例在跑/正在起」就算开着，按钮给「停止」。
                    // 只看 ready 的话，启动中（最长 30s）和退避重试期间按钮还是「启动」，
                    // 再点一次就会又起一路监听 —— 这正是「一个项目出现多个监听」的来源。
                    const live =
                      status?.state === "starting" ||
                      status?.state === "running" ||
                      status?.state === "backing_off";
                    const active = live;
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
                  {/* 监听范围：留空就是「所有群、所有人」，要说清楚 */}
                  <span style={{ marginLeft: "6px", color: "var(--text-secondary)" }}>
                    · 监听范围：
                    {project.group_ids.length === 0 && project.member_ids.length === 0
                      ? "所有群 / 所有人"
                      : `${project.group_ids.length} 个群 / ${project.member_ids.length} 个人`}
                  </span>
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
                          className="amx-row"
                          onClick={() => onSelectSession(session, project)}
                          style={{
                            padding: "5px 12px 5px 28px",
                            cursor: "pointer",
                            backgroundColor: selected ? "var(--bg-active)" : "transparent",
                          }}
                        >
                          <div
                            style={{
                              display: "flex",
                              alignItems: "center",
                              gap: "6px",
                              fontSize: "12px",
                              color: "var(--text-primary)",
                            }}
                          >
                            <KindTag kind={session.kind} />
                            <span
                              style={{
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                              }}
                            >
                              {session.name}
                            </span>
                            <button
                              className="amx-row-actions"
                              title="更多操作"
                              onClick={(e) => {
                                // 别把点击冒泡成「选中会话」：删完就跳进空会话很困惑。
                                e.stopPropagation();
                                openRowMenu(e, [
                                  {
                                    label: "删除会话",
                                    danger: true,
                                    onClick: () => onDeleteSession(session),
                                  },
                                ]);
                              }}
                              style={{ ...iconButton, marginLeft: "auto", lineHeight: 1, fontSize: "14px" }}
                            >
                              ⋯
                            </button>
                          </div>
                          {/* 名字已知时不再重复显示会话 id，省一行视觉噪音 */}
                          {!session.name_known && (
                            <div
                              style={{
                                fontSize: "10px",
                                color: "var(--text-muted)",
                              }}
                            >
                              名称未知，显示会话 id
                            </div>
                          )}
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
                className="amx-row"
                style={{
                  padding: "6px 12px 8px",
                  backgroundColor: selected ? "var(--bg-active)" : "transparent",
                }}
              >
                <div
                  onClick={() => onSelectSession(session, null)}
                  style={{ cursor: "pointer" }}
                >
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: "6px",
                      fontSize: "12px",
                      color: "var(--text-primary)",
                    }}
                  >
                    <KindTag kind={session.kind} />
                    <span
                      style={{
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {session.name}
                    </span>
                    <button
                      className="amx-row-actions"
                      title="更多操作"
                      onClick={(e) => {
                        e.stopPropagation();
                        openRowMenu(e, [
                          {
                            label: "删除会话",
                            danger: true,
                            onClick: () => onDeleteSession(session),
                          },
                        ]);
                      }}
                      style={{ ...iconButton, marginLeft: "auto", lineHeight: 1, fontSize: "14px" }}
                    >
                      ⋯
                    </button>
                  </div>
                  {!session.name_known && (
                    <div style={{ fontSize: "10px", color: "var(--text-muted)" }}>
                      名称未知，显示会话 id
                    </div>
                  )}
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

      {/* 行操作菜单：fixed 定位 + 一层透明遮罩（点别处即关） */}
      {menu && (
        <>
          <div
            onClick={() => setMenu(null)}
            style={{ position: "fixed", inset: 0, zIndex: 40 }}
          />
          <div
            style={{
              position: "fixed",
              top: menu.top,
              left: menu.left,
              zIndex: 41,
              width: "132px",
              padding: "4px",
              display: "flex",
              flexDirection: "column",
              gap: "2px",
              backgroundColor: "var(--bg-elevated)",
              border: "1px solid var(--border)",
              borderRadius: "6px",
              boxShadow: "var(--shadow)",
            }}
          >
            {menu.items.map((item) => (
              <button
                key={item.label}
                onClick={() => {
                  setMenu(null);
                  item.onClick();
                }}
                style={{
                  padding: "5px 10px",
                  fontSize: "12px",
                  textAlign: "left",
                  backgroundColor: "transparent",
                  color: item.danger ? "var(--danger)" : "var(--text-secondary)",
                  border: "none",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                {item.label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
