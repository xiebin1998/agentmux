import { useCallback, useEffect, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import ProjectList from "./components/ProjectList";
import SessionList from "./components/SessionList";
import MessageView from "./components/MessageView";
import ContextPanel from "./components/ContextPanel";
import ProjectDialog from "./components/ProjectDialog";
import EventStream from "./components/EventStream";
import ListenerLogs from "./components/ListenerLogs";
import ReplyHistory from "./components/ReplyHistory";
import ProvidersView from "./components/ProvidersView";
import OverviewView from "./components/OverviewView";
import SettingsPanel from "./components/SettingsPanel";
import PluginsPanel from "./components/PluginsPanel";
import { ThemeToggle } from "./theme";

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

type ListenerState = "stopped" | "starting" | "running" | "backing_off" | "abandoned";

interface ListenerStatus {
  id: string;
  kind: ListenKind;
  state: ListenerState;
  ready: boolean;
  subscribe_id: string | null;
  bus_pid: number | null;
  attempts: number;
  last_error: string | null;
  cli_path: string | null;
  dropped_before_ready: number;
}

interface ConversationSummary {
  conversation_id: string;
  events: number;
  last_sender: string;
  last_received_at: string;
  replied: number;
}

type ListenerUpdate =
  | { type: "status"; status: ListenerStatus }
  | { type: "event"; event: unknown }
  | { type: "log"; listener_id: string; line: string };

const KINDS: { kind: ListenKind; label: string }[] = [
  { kind: "at-me", label: "@我" },
  { kind: "all-direct", label: "单聊" },
];

type View = "overview" | "events" | "replies" | "logs" | "providers";

const VIEWS: { view: View; label: string }[] = [
  { view: "overview", label: "运行总览" },
  { view: "events", label: "实时事件流" },
  { view: "replies", label: "回复历史" },
  { view: "logs", label: "监听日志" },
  { view: "providers", label: "提供方检测" },
];

function stateText(status: ListenerStatus | undefined) {
  if (!status) return "未启动";
  if (status.ready) return "监听中";
  switch (status.state) {
    case "starting":
      return "启动中";
    case "backing_off":
      return "退避重试中";
    case "abandoned":
      return "已放弃";
    case "stopped":
      return "已停止";
    default:
      return "未启动";
  }
}

function stateColor(status: ListenerStatus | undefined) {
  if (!status) return "var(--text-muted)";
  if (status.ready) return "var(--success)";
  if (status.state === "abandoned") return "var(--danger)";
  if (status.state === "starting" || status.state === "backing_off") return "var(--warn)";
  return "var(--text-muted)";
}

function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [showProjectDialog, setShowProjectDialog] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showPlugins, setShowPlugins] = useState(false);
  const [view, setView] = useState<View>("overview");
  const [editingProject, setEditingProject] = useState<Project | null>(null);
  const [statuses, setStatuses] = useState<Record<string, ListenerStatus>>({});
  const [refreshToken, setRefreshToken] = useState(0);
  const [busyKind, setBusyKind] = useState<string | null>(null);
  const [banner, setBanner] = useState<string | null>(null);

  const loadProjects = useCallback(async () => {
    try {
      const result = await invoke<Project[]>("list_projects");
      setProjects(result);
      setSelectedProject((current) =>
        current ? (result.find((p) => p.id === current.id) ?? null) : null,
      );
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }, []);

  const loadConversations = useCallback(async () => {
    try {
      const result = await invoke<ConversationSummary[]>("list_conversations");
      setConversations(result);
    } catch (e) {
      console.error("Failed to load conversations:", e);
    }
  }, []);

  const loadStatuses = useCallback(async () => {
    try {
      const result = await invoke<ListenerStatus[]>("listener_status");
      const next: Record<string, ListenerStatus> = {};
      for (const status of result) {
        next[status.kind] = status;
      }
      setStatuses(next);
    } catch (e) {
      console.error("Failed to load listener status:", e);
    }
  }, []);

  useEffect(() => {
    loadProjects();
    loadConversations();
    loadStatuses();
    const timer = setInterval(() => {
      loadConversations();
      loadStatuses();
    }, 3000);
    return () => clearInterval(timer);
  }, [loadProjects, loadConversations, loadStatuses]);

  const handleStart = async (kind: ListenKind) => {
    setBusyKind(kind);
    setBanner(null);
    try {
      const channel = new Channel<ListenerUpdate>();
      channel.onmessage = (message) => {
        if (message.type === "status") {
          setStatuses((prev) => ({ ...prev, [message.status.kind]: message.status }));
        } else if (message.type === "event") {
          setRefreshToken((token) => token + 1);
        }
      };
      await invoke<string>("start_listener", { kind, channel });
      await loadStatuses();
    } catch (e) {
      setBanner(String(e));
    } finally {
      setBusyKind(null);
    }
  };

  const handleStop = async (kind: ListenKind) => {
    const status = statuses[kind];
    if (!status) return;
    setBusyKind(kind);
    try {
      await invoke("stop_listener", { id: status.id });
      await loadStatuses();
    } catch (e) {
      setBanner(String(e));
    } finally {
      setBusyKind(null);
    }
  };

  const handleDeleteProject = async (id: string) => {
    if (!confirm("确定要删除这个项目吗？")) return;
    try {
      await invoke("delete_project", { id });
      await loadProjects();
      if (selectedProject?.id === id) {
        setSelectedProject(null);
        setSelectedSession(null);
      }
    } catch (e) {
      console.error("Failed to delete project:", e);
    }
  };

  const sessions: Session[] = conversations.map((conversation) => ({
    id: conversation.conversation_id,
    project_id: selectedProject?.id ?? "",
    name: conversation.last_sender
      ? `${conversation.last_sender}（${conversation.events} 条）`
      : conversation.conversation_id,
    conversation_id: conversation.conversation_id,
    created_at: conversation.last_received_at,
  }));

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100vh" }}>
      <header
        style={{
          minHeight: "48px",
          backgroundColor: "var(--bg-sidebar)",
          color: "var(--text-primary)",
          display: "flex",
          alignItems: "center",
          gap: "12px",
          padding: "8px 16px",
          borderBottom: "1px solid var(--border)",
          flexWrap: "wrap",
        }}
      >
        <h1 style={{ margin: 0, fontSize: "16px", fontWeight: 600 }}>AgentMux</h1>

        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          {KINDS.map(({ kind, label }) => {
            const status = statuses[kind];
            const active = Boolean(status?.ready);
            const busy = busyKind === kind;
            return (
              <div
                key={kind}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "6px",
                  padding: "3px 8px",
                  border: "1px solid var(--border)",
                  borderRadius: "6px",
                }}
              >
                <span style={{ fontSize: "12px", color: "var(--text-secondary)" }}>{label}</span>
                <span style={{ fontSize: "11px", color: stateColor(status) }}>
                  {stateText(status)}
                </span>
                <button
                  onClick={() => (active ? handleStop(kind) : handleStart(kind))}
                  disabled={busy}
                  style={{
                    padding: "2px 8px",
                    fontSize: "11px",
                    border: "none",
                    borderRadius: "4px",
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

        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: "8px" }}>
          <ThemeToggle />
          <button
            onClick={() => setShowPlugins(true)}
            style={{
              padding: "6px 12px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              cursor: "pointer",
              fontSize: "13px",
            }}
          >
            插件
          </button>
          <button
            onClick={() => setShowSettings(true)}
            style={{
              padding: "6px 12px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              cursor: "pointer",
              fontSize: "13px",
            }}
          >
            设置
          </button>
          <button
            onClick={() => {
              setEditingProject(null);
              setShowProjectDialog(true);
            }}
            style={{
              padding: "6px 12px",
              backgroundColor: "var(--accent)",
              color: "var(--accent-contrast)",
              border: "none",
              borderRadius: "4px",
              cursor: "pointer",
              fontSize: "13px",
            }}
          >
            创建项目
          </button>
        </div>
      </header>

      {banner && (
        <div
          style={{
            padding: "6px 16px",
            backgroundColor: "var(--danger-strong)",
            color: "#fff",
            fontSize: "12px",
          }}
        >
          {banner}
        </div>
      )}

      <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
        <div
          style={{
            width: "280px",
            backgroundColor: "var(--bg-sidebar)",
            borderRight: "1px solid var(--border)",
            display: "flex",
            flexDirection: "column",
            overflow: "auto",
          }}
        >
          <ProjectList
            projects={projects}
            selectedProject={selectedProject}
            onSelectProject={setSelectedProject}
            onEditProject={(project) => {
              setEditingProject(project);
              setShowProjectDialog(true);
            }}
            onDeleteProject={handleDeleteProject}
          />
          <SessionList
            sessions={sessions}
            selectedSession={selectedSession}
            onSelectSession={setSelectedSession}
          />
        </div>

        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            backgroundColor: "var(--bg-app)",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              display: "flex",
              gap: "4px",
              padding: "6px 12px",
              borderBottom: "1px solid var(--border)",
              backgroundColor: "var(--bg-sidebar)",
            }}
          >
            {VIEWS.map(({ view: option, label }) => {
              const active = !selectedSession && view === option;
              return (
                <button
                  key={option}
                  onClick={() => {
                    setSelectedSession(null);
                    setView(option);
                  }}
                  style={{
                    padding: "4px 10px",
                    fontSize: "12px",
                    border: "none",
                    borderRadius: "4px",
                    cursor: "pointer",
                    backgroundColor: active ? "var(--bg-active)" : "transparent",
                    color: active ? "var(--text-primary)" : "var(--text-secondary)",
                  }}
                >
                  {label}
                </button>
              );
            })}
            {selectedSession && (
              <span style={{ marginLeft: "auto", fontSize: "11px", color: "var(--text-muted)" }}>
                正在查看会话，点上方任一视图可返回
              </span>
            )}
          </div>

          <div style={{ flex: 1, overflow: "hidden" }}>
            {selectedSession ? (
              <MessageView session={selectedSession} />
            ) : view === "overview" ? (
              <OverviewView />
            ) : view === "events" ? (
              <EventStream refreshToken={refreshToken} />
            ) : view === "replies" ? (
              <ReplyHistory />
            ) : view === "logs" ? (
              <ListenerLogs />
            ) : (
              <ProvidersView />
            )}
          </div>
        </div>

        {selectedSession && (
          <ContextPanel session={selectedSession} project={selectedProject} />
        )}
      </div>

      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}
      {showPlugins && <PluginsPanel onClose={() => setShowPlugins(false)} />}

      {showProjectDialog && (
        <ProjectDialog
          project={editingProject}
          onClose={() => setShowProjectDialog(false)}
          onSaved={async () => {
            setShowProjectDialog(false);
            await loadProjects();
          }}
        />
      )}
    </div>
  );
}

export default App;
