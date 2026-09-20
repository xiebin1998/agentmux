import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
}

interface ProjectOption {
  id: string;
  name: string;
}

interface Stats {
  total_events: number;
  malformed_events: number;
  processed_events: number;
  replied_events: number;
  failed_replies: number;
  conversations: number;
}

/** conversation_details 命令的返回里，右列用得上的部分。 */
interface ConversationDetails {
  model: string | null;
  model_override: string | null;
  reasoning_effort_override: string | null;
}

type ListenerState = "stopped" | "starting" | "running" | "backing_off" | "abandoned";

interface ListenerStatus {
  id: string;
  project_id: string;
  kind: string;
  state: ListenerState;
  ready: boolean;
  attempts: number;
  last_error: string | null;
}

const KIND_LABEL: Record<string, string> = { group: "群聊", direct: "单聊" };

const EFFORT_LABELS: Record<string, string> = {
  low: "低",
  medium: "中",
  high: "高",
};

interface ContextPanelProps {
  /** 选中会话时右列才有模型/思考强度（它们读的是当前会话的覆盖值）。 */
  session: Session | null;
  project: ProjectOption | null;
  projects: ProjectOption[];
  onOpenSettings: () => void;
}

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-secondary)",
  marginBottom: "10px",
};

const rowStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  gap: "10px",
  fontSize: "12px",
};

const selectStyle: CSSProperties = {
  flex: 1,
  minWidth: 0,
  padding: "4px 6px",
  fontSize: "12px",
  backgroundColor: "var(--bg-app)",
  color: "var(--text-primary)",
  border: "1px solid var(--border)",
  borderRadius: "4px",
};

function stateLabel(status: ListenerStatus) {
  if (status.state === "running" && status.ready) {
    return { text: "监听中", color: "var(--success)" };
  }
  switch (status.state) {
    case "starting":
      return { text: "启动中", color: "var(--warn)" };
    case "backing_off":
      return { text: "退避重试中", color: "var(--warn)" };
    case "abandoned":
      return { text: "已放弃", color: "var(--danger)" };
    case "stopped":
      return { text: "已停止", color: "var(--text-muted)" };
    default:
      return { text: status.state, color: "var(--text-secondary)" };
  }
}

/**
 * 右列：运行信息。
 *
 * 只放**别处没有的**东西 —— 监听状态、运行统计，以及选中会话时的模型/思考强度。
 * 项目配置、回复配置、会话操作都不在这里：项目弹窗与会话窗口的「⋯」已经有入口，
 * 两处编辑同一份设置只会让人搞不清改了哪个。
 */
export default function ContextPanel({
  session,
  project,
  projects,
  onOpenSettings,
}: ContextPanelProps) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [listeners, setListeners] = useState<ListenerStatus[]>([]);
  const [details, setDetails] = useState<ConversationDetails | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [nextStats, nextListeners] = await Promise.all([
          invoke<Stats>("get_stats"),
          invoke<ListenerStatus[]>("listener_status"),
        ]);
        if (!cancelled) {
          setStats(nextStats);
          setListeners(nextListeners);
        }
      } catch (e) {
        console.error("Failed to load run info:", e);
      }
    };
    load();
    const timer = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (!project || !session) {
      setDetails(null);
      return;
    }
    let cancelled = false;
    const load = async () => {
      try {
        const next = await invoke<ConversationDetails>("conversation_details", {
          projectId: project.id,
          conversationId: session.conversation_id,
        });
        if (!cancelled) setDetails(next);
      } catch (e) {
        console.error("Failed to load conversation details:", e);
      }
    };
    load();
    // 当前模型/占比都是「最近一次生成」的快照，回复后要跟着变。
    const timer = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [project, session]);

  useEffect(() => {
    if (!project) {
      setModels([]);
      return;
    }
    let cancelled = false;
    // 可选模型列表来自 CLI，切项目时拉一次即可，不轮询。
    invoke<string[]>("list_agent_models", { projectId: project.id })
      .then((list) => {
        if (!cancelled) setModels(list);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [project]);

  const nameOf = (projectId: string) =>
    projects.find((item) => item.id === projectId)?.name ?? "未归属项目";

  // 终态监听不占版面：它已经在左侧项目行里显示成「未启动」了。
  const live = listeners.filter(
    (item) => !(item.state === "stopped" || item.state === "abandoned"),
  );

  const pickModel = async (model: string) => {
    try {
      await invoke("set_agent_model", { model });
      setNotice(model ? `已切到 ${model}，下一条消息生效` : "已恢复 CLI 默认模型");
    } catch (e) {
      setNotice(`切换失败：${e}`);
    }
  };

  const pickEffort = async (level: string) => {
    try {
      await invoke("set_reasoning_effort", { level });
      setNotice(
        level ? `已切到「${EFFORT_LABELS[level] ?? level}」，下一条消息生效` : "已恢复 CLI 默认强度",
      );
    } catch (e) {
      setNotice(`切换失败：${e}`);
    }
  };

  return (
    <div
      style={{
        width: "260px",
        backgroundColor: "var(--bg-sidebar)",
        borderLeft: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        overflow: "auto",
      }}
    >
      <div style={{ padding: "14px 16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>监听状态</div>
        {live.length === 0 ? (
          <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>
            没有在跑的监听
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
            {live.map((listener) => {
              const label = stateLabel(listener);
              return (
                <div key={listener.id} style={{ fontSize: "12px" }}>
                  <div style={{ display: "flex", justifyContent: "space-between", gap: "8px" }}>
                    <span
                      style={{
                        color: "var(--text-secondary)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {nameOf(listener.project_id)} · {KIND_LABEL[listener.kind] ?? listener.kind}
                    </span>
                    <span style={{ color: label.color, whiteSpace: "nowrap" }}>{label.text}</span>
                  </div>
                  {listener.last_error && (
                    <div style={{ color: "var(--danger)", fontSize: "11px", marginTop: "2px" }}>
                      {listener.last_error}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div style={{ padding: "14px 16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>运行统计</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>事件</span>
            <span style={{ color: "var(--text-primary)" }}>
              {stats?.total_events ?? "—"}
              {stats?.malformed_events ? (
                <span style={{ color: "var(--warn)" }}> · 畸形 {stats.malformed_events}</span>
              ) : null}
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>已回复</span>
            <span style={{ color: "var(--text-primary)" }}>{stats?.replied_events ?? "—"}</span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>回复失败</span>
            <span style={{ color: stats?.failed_replies ? "var(--danger)" : "var(--text-primary)" }}>
              {stats?.failed_replies ?? "—"}
            </span>
          </div>
        </div>
      </div>

      {project && session && (
        <div style={{ padding: "14px 16px", borderBottom: "1px solid var(--border)" }}>
          <div style={sectionTitle}>模型</div>
          <div style={{ fontSize: "12px", color: "var(--text-muted)", marginBottom: "8px" }}>
            当前：{details?.model ?? "回复过一次后由 Agent 回报"}
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
            <select
              value={details?.model_override ?? ""}
              onChange={(e) => pickModel(e.target.value)}
              style={selectStyle}
              title="模型"
            >
              <option value="">CLI 默认模型</option>
              {models.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
            <select
              value={details?.reasoning_effort_override ?? ""}
              onChange={(e) => pickEffort(e.target.value)}
              style={selectStyle}
              title="思考强度（越高越慢）"
            >
              <option value="">思考强度：CLI 默认</option>
              {Object.entries(EFFORT_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  思考强度：{label}
                </option>
              ))}
            </select>
          </div>
          {notice && (
            <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginTop: "6px" }}>
              {notice}
            </div>
          )}
        </div>
      )}

      <div style={{ padding: "14px 16px", marginTop: "auto" }}>
        <button
          onClick={onOpenSettings}
          style={{
            width: "100%",
            padding: "6px 10px",
            fontSize: "12px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
          }}
        >
          更多设置…
        </button>
      </div>
    </div>
  );
}