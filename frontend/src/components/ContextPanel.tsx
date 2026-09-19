import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import CompressionSection from "./CompressionSection";

interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
}

interface Project {
  id: string;
  name: string;
  work_dir: string;
  agent_cli_path: string;
  dingtalk_cli_path: string;
  agent_platform?: string;
  im_platform?: string;
  reply_enabled: boolean;
  reply_timeout_ms: number;
  reply_max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
}

interface Stats {
  total_events: number;
  malformed_events: number;
  processed_events: number;
  replied_events: number;
  failed_replies: number;
  conversations: number;
}

interface ContextPanelProps {
  session: Session;
  project: Project | null;
}

const KIND_LABEL: Record<string, string> = { group: "群聊", direct: "单聊" };

/** conversation_details 命令的返回：会话名/类型 + 上下文用量 + 当前模型。 */
interface ConversationDetails {
  conversation_id: string;
  name: string;
  kind: string;
  context_budget_chars: number;
  context_used_chars: number;
  context_message_limit: number;
  /** Agent 最近一次回报的真实占用比例（0~1）；没跑过为 null */
  context_usage_ratio: number | null;
  compress_trigger_percent: number | null;
  /** Agent 最近一次实际用的模型 */
  model: string | null;
  /** 配置里显式指定的模型；空 = 用 CLI 默认 */
  model_override: string | null;
}

interface AgentSessionInfo {
  conversation_id: string;
  agent_session_id: string;
  agent_cwd: string;
}

type ListenerState = "stopped" | "starting" | "running" | "backing_off" | "abandoned";

interface ListenerStatus {
  id: string;
  kind: string;
  state: ListenerState;
  ready: boolean;
  subscribe_id: string | null;
  bus_pid: number | null;
  attempts: number;
  last_error: string | null;
  cli_path: string | null;
  dropped_before_ready: number;
}

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-secondary)",
  textTransform: "uppercase",
  marginBottom: "12px",
};

const rowStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  gap: "12px",
  fontSize: "12px",
};

function stateLabel(status: ListenerStatus | undefined) {
  if (!status) return { text: "未启动", color: "var(--text-muted)" };
  if (status.state === "running" && status.ready) return { text: "监听中", color: "var(--success)" };
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

export default function ContextPanel({ session, project }: ContextPanelProps) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [listeners, setListeners] = useState<ListenerStatus[]>([]);
  const [sessionNotice, setSessionNotice] = useState<string | null>(null);
  const [agentSession, setAgentSession] = useState<AgentSessionInfo | null>(null);
  const [details, setDetails] = useState<ConversationDetails | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [modelNotice, setModelNotice] = useState<string | null>(null);

  // 会话窗口要显示的上下文用量与当前模型；跟着会话切换刷新。
  useEffect(() => {
    if (!project) return;
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
    // 上下文占比/模型都是「最近一次生成」的快照，回复后要跟着变，所以轮询。
    const timer = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [project, session.conversation_id]);

  useEffect(() => {
    if (!project) return;
    let cancelled = false;
    // 可选模型列表来自 CLI，启动/切会话时拉一次即可，不轮询。
    invoke<string[]>("list_agent_models", { projectId: project.id })
      .then((list) => {
        if (!cancelled) setModels(list);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [project]);

  const handlePickModel = async (model: string) => {
    try {
      await invoke("set_agent_model", { model });
      setModelNotice(
        model ? `已切换到 ${model}（下一条消息生效）` : "已恢复用 CLI 默认模型（下一条消息生效）",
      );
    } catch (e) {
      setModelNotice(`切换失败：${e}`);
    }
  };

  useEffect(() => {
    if (!project) return;
    let cancelled = false;
    invoke<AgentSessionInfo | null>("conversation_session", {
      projectId: project.id,
      conversationId: session.conversation_id,
    })
      .then((result) => {
        if (!cancelled) setAgentSession(result);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [project, session.conversation_id, sessionNotice]);

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
        console.error("Failed to load panel data:", e);
      }
    };
    load();
    const timer = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [session.id]);

  if (!project) return null;

  return (
    <div
      style={{
        width: "320px",
        backgroundColor: "var(--bg-sidebar)",
        borderLeft: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        overflow: "auto",
      }}
    >
      <div style={{ padding: "16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>运行统计</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>累计事件</span>
            <span style={{ color: "var(--text-primary)" }}>{stats?.total_events ?? "—"}</span>
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
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>畸形事件</span>
            <span
              style={{ color: stats?.malformed_events ? "var(--warn)" : "var(--text-primary)" }}
            >
              {stats?.malformed_events ?? "—"}
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>会话数</span>
            <span style={{ color: "var(--text-primary)" }}>{stats?.conversations ?? "—"}</span>
          </div>
        </div>
      </div>

      <div style={{ padding: "16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>监听状态</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
          {listeners.length === 0 ? (
            <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>尚未启动监听</div>
          ) : (
            listeners.map((listener) => {
              const label = stateLabel(listener);
              return (
                <div key={listener.id} style={{ fontSize: "12px" }}>
                  <div style={{ display: "flex", justifyContent: "space-between" }}>
                    <span style={{ color: "var(--text-secondary)" }}>{listener.kind}</span>
                    <span style={{ color: label.color }}>{label.text}</span>
                  </div>
                  <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "2px" }}>
                    {listener.subscribe_id ?? "无订阅 id"} · bus_pid {listener.bus_pid ?? "—"}
                  </div>
                  {listener.last_error && (
                    <div style={{ color: "var(--danger)", fontSize: "11px", marginTop: "2px" }}>
                      {listener.last_error}
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      </div>

      <div style={{ padding: "16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>项目配置</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "10px", fontSize: "12px" }}>
          <div>
            <div style={{ color: "var(--text-secondary)", marginBottom: "4px" }}>工作目录</div>
            <div style={{ color: "var(--text-primary)", wordBreak: "break-all" }}>
              {project.work_dir}
            </div>
          </div>
          <div>
            <div style={{ color: "var(--text-secondary)", marginBottom: "4px" }}>
              Agent CLI（{project.agent_platform ?? "—"}）
            </div>
            <div
              style={{
                color: "var(--text-primary)",
                wordBreak: "break-all",
                fontSize: "11px",
                fontFamily: "ui-monospace, Consolas, monospace",
              }}
            >
              {project.agent_cli_path}
            </div>
          </div>
          <div>
            <div style={{ color: "var(--text-secondary)", marginBottom: "4px" }}>
              IM CLI（{project.im_platform ?? "—"}）
            </div>
            <div
              style={{
                color: "var(--text-primary)",
                wordBreak: "break-all",
                fontSize: "11px",
                fontFamily: "ui-monospace, Consolas, monospace",
              }}
            >
              {project.dingtalk_cli_path}
            </div>
          </div>
        </div>
      </div>

      <div style={{ padding: "16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>回复配置</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>自动回复</span>
            <span
              style={{ color: project.reply_enabled ? "var(--success)" : "var(--text-muted)" }}
            >
              {project.reply_enabled ? "已启用" : "已禁用"}
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>生成超时</span>
            <span style={{ color: "var(--text-primary)" }}>
              {project.reply_timeout_ms / 1000}s
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>字数上限</span>
            <span style={{ color: "var(--text-primary)" }}>{project.reply_max_chars} 字符</span>
          </div>
        </div>
      </div>

      <div style={{ padding: "16px" }}>
        <div style={sectionTitle}>上下文预算</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>消息条数</span>
            <span style={{ color: "var(--text-primary)" }}>
              {project.context_message_limit} 条
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>字符预算</span>
            <span style={{ color: "var(--text-primary)" }}>{project.context_max_chars} 字符</span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>状态</span>
            <span
              style={{ color: project.context_enabled ? "var(--success)" : "var(--text-muted)" }}
            >
              {project.context_enabled ? "已启用" : "已禁用"}
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>当前累计</span>
            <span style={{ color: "var(--text-primary)" }}>
              {details ? `${details.context_used_chars} 字符` : "—"}
            </span>
          </div>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>实际占用</span>
            <span
              style={{
                color:
                  details?.context_usage_ratio != null &&
                  details.compress_trigger_percent != null &&
                  details.context_usage_ratio * 100 >= details.compress_trigger_percent
                    ? "var(--warn)"
                    : "var(--text-primary)",
              }}
            >
              {details?.context_usage_ratio != null
                ? `${(details.context_usage_ratio * 100).toFixed(2)}%`
                : "暂无（回复过一次后由 Agent 回报）"}
            </span>
          </div>
          {details?.context_usage_ratio != null && (
            <div
              style={{
                height: "6px",
                borderRadius: "3px",
                backgroundColor: "var(--bg-active)",
                overflow: "hidden",
              }}
              title={`压缩阈值 ${details.compress_trigger_percent ?? "未设"}%`}
            >
              <div
                style={{
                  height: "100%",
                  width: `${Math.min(100, details.context_usage_ratio * 100).toFixed(1)}%`,
                  backgroundColor:
                    details.compress_trigger_percent != null &&
                    details.context_usage_ratio * 100 >= details.compress_trigger_percent
                      ? "var(--warn)"
                      : "var(--accent)",
                }}
              />
            </div>
          )}
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>压缩阈值</span>
            <span style={{ color: "var(--text-muted)" }}>
              {details?.compress_trigger_percent != null
                ? `用到 ${details.compress_trigger_percent}% 时压缩`
                : "未设置"}
            </span>
          </div>
        </div>
      </div>

      <div style={{ padding: "16px", borderBottom: "1px solid var(--border)" }}>
        <div style={sectionTitle}>模型</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>当前使用</span>
            <span
              style={{
                color: "var(--text-primary)",
                wordBreak: "break-all",
                textAlign: "right",
              }}
            >
              {details?.model ?? "暂无（回复过一次后由 Agent 回报）"}
            </span>
          </div>
          <div style={{ display: "flex", gap: "6px", alignItems: "center" }}>
            <span style={{ fontSize: "12px", color: "var(--text-secondary)", whiteSpace: "nowrap" }}>
              切换为
            </span>
            <select
              value={details?.model_override ?? ""}
              onChange={(e) => handlePickModel(e.target.value)}
              style={{
                flex: 1,
                minWidth: 0,
                padding: "4px 6px",
                fontSize: "12px",
                backgroundColor: "var(--bg-app)",
                color: "var(--text-primary)",
                border: "1px solid var(--border)",
                borderRadius: "4px",
              }}
            >
              <option value="">CLI 默认模型</option>
              {models.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
          <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
            {models.length === 0
              ? "读不到可选模型列表（该 CLI 不支持 --list-models），可只用默认模型"
              : "切换写进全局设置，下一条消息生效；重启监听不需要"}
            {modelNotice ? ` · ${modelNotice}` : ""}
          </div>
        </div>
      </div>
      <CompressionSection
        projectId={project.id}
        conversationId={session.conversation_id}
      />

      <div style={{ padding: "16px", borderTop: "1px solid var(--border)" }}>
        <div style={sectionTitle}>会话</div>
        <div style={{ display: "flex", flexDirection: "column", gap: "6px", marginBottom: "10px" }}>
          <div style={rowStyle}>
            <span style={{ color: "var(--text-secondary)" }}>类型</span>
            <span
              style={{
                color:
                  details?.kind === "group"
                    ? "var(--accent)"
                    : details?.kind === "direct"
                      ? "var(--success)"
                      : "var(--text-muted)",
              }}
            >
              {KIND_LABEL[details?.kind ?? ""] ?? "未知（尚未同步）"}
            </span>
          </div>
          <div style={{ ...rowStyle, alignItems: "flex-start" }}>
            <span style={{ color: "var(--text-secondary)" }}>名称</span>
            <span
              style={{
                color: details?.name ? "var(--text-primary)" : "var(--text-muted)",
                textAlign: "right",
                wordBreak: "break-all",
              }}
            >
              {details?.name || session.name}
            </span>
          </div>
        </div>
        <button
          onClick={async () => {
            try {
              const count = await invoke<number>("refresh_conversation_meta");
              const next = await invoke<ConversationDetails>("conversation_details", {
                projectId: project.id,
                conversationId: session.conversation_id,
              });
              setDetails(next);
              setSessionNotice(`已同步 ${count} 个会话的名称与类型`);
            } catch (e) {
              setSessionNotice(`同步失败：${e}`);
            }
          }}
          style={{
            marginBottom: "10px",
            padding: "4px 10px",
            fontSize: "12px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
          }}
        >
          刷新会话信息（群名/单聊名）
        </button>
        <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "10px" }}>
          Agent 会话状态：首次回复时按 conversation_id 建档，之后按会话续接。
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: "6px", marginBottom: "10px" }}>
          <div style={{ display: "flex", justifyContent: "space-between", fontSize: "12px" }}>
            <span style={{ color: "var(--text-secondary)" }}>状态</span>
            <span style={{ color: agentSession ? "var(--success)" : "var(--text-muted)" }}>
              {agentSession ? "已建档" : "未建档（下一条消息会新建）"}
            </span>
          </div>
          {agentSession && (
            <>
              <div style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
                Agent 会话 id
                <div
                  style={{
                    fontFamily: "ui-monospace, Consolas, monospace",
                    color: "var(--text-primary)",
                    wordBreak: "break-all",
                  }}
                >
                  {agentSession.agent_session_id}
                </div>
              </div>
              <div style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
                建档工作目录
                <div
                  style={{
                    fontFamily: "ui-monospace, Consolas, monospace",
                    color: "var(--text-primary)",
                    wordBreak: "break-all",
                  }}
                >
                  {agentSession.agent_cwd}
                </div>
              </div>
            </>
          )}
        </div>
        <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "10px" }}>
          作废该会话的 Agent 记录：下一条消息将重新建档并开新会话，
          历史事件与回复记录不受影响。
        </div>
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
                projectId: project.id,
                conversationId: session.conversation_id,
              });
              setSessionNotice("已作废，下一条消息将重建会话");
            } catch (e) {
              setSessionNotice("作废失败：" + String(e));
            }
          }}
          style={{
            padding: "5px 12px",
            fontSize: "12px",
            backgroundColor: "transparent",
            color: "var(--danger)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
          }}
        >
          作废并重建会话
        </button>
        {sessionNotice && (
          <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginTop: "8px" }}>
            {sessionNotice}
          </div>
        )}
      </div>
    </div>
  );
}
