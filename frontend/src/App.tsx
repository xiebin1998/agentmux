import { useCallback, useEffect, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ProjectList from "./components/ProjectList";
import MessageView from "./components/MessageView";
import ContextPanel from "./components/ContextPanel";
import ProjectDialog from "./components/ProjectDialog";
import EventStream from "./components/EventStream";
import ListenerLogs from "./components/ListenerLogs";
import ReplyHistory from "./components/ReplyHistory";
import ProvidersView from "./components/ProvidersView";
import OverviewView from "./components/OverviewView";
import SettingsPanel from "./components/SettingsPanel";
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
  project_id: string;
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

type View = "overview" | "events" | "replies" | "logs" | "providers";

const VIEWS: { view: View; label: string }[] = [
  { view: "overview", label: "运行总览" },
  { view: "events", label: "实时事件流" },
  { view: "replies", label: "回复历史" },
  { view: "logs", label: "监听日志" },
  { view: "providers", label: "提供方检测" },
];

function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [sessionsByProject, setSessionsByProject] = useState<Record<string, Session[]>>({});
  const [statusesByProject, setStatusesByProject] = useState<
    Record<string, Partial<Record<ListenKind, ListenerStatus>>>
  >({});
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [showProjectDialog, setShowProjectDialog] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [view, setView] = useState<View>("overview");
  const [editingProject, setEditingProject] = useState<Project | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [banner, setBanner] = useState<string | null>(null);
  const [showCloseDialog, setShowCloseDialog] = useState(false);

  // 点 × 时后端拦下关闭并通知前端，由用户选「最小化到托盘」还是「退出」。
  useEffect(() => {
    const pending = listen("close-requested", () => setShowCloseDialog(true));
    return () => {
      pending.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  const loadProjects = useCallback(async () => {
    try {
      const result = await invoke<Project[]>("list_projects");
      setProjects(result);
      setSelectedProject((current) =>
        current ? (result.find((p) => p.id === current.id) ?? null) : null,
      );
      return result;
    } catch (e) {
      console.error("Failed to load projects:", e);
      return [];
    }
  }, []);

  /** 会话按项目分别拉取：左树是「项目 → 会话」两层。 */
  const loadSessions = useCallback(async (list: Project[]) => {
    const next: Record<string, Session[]> = {};
    await Promise.all(
      list.map(async (project) => {
        try {
          const conversations = await invoke<ConversationSummary[]>("list_conversations", {
            projectId: project.id,
          });
          next[project.id] = conversations.map((conversation) => ({
            id: conversation.conversation_id,
            project_id: project.id,
            name: conversation.last_sender
              ? `${conversation.last_sender}（${conversation.events} 条）`
              : conversation.conversation_id,
            conversation_id: conversation.conversation_id,
            created_at: conversation.last_received_at,
          }));
        } catch (e) {
          console.error("Failed to load conversations for", project.id, e);
          next[project.id] = [];
        }
      }),
    );
    setSessionsByProject(next);
  }, []);

  const loadStatuses = useCallback(async () => {
    try {
      const list = await invoke<ListenerStatus[]>("listener_status");
      const next: Record<string, Partial<Record<ListenKind, ListenerStatus>>> = {};
      for (const status of list) {
        next[status.project_id] = { ...(next[status.project_id] ?? {}), [status.kind]: status };
      }
      setStatusesByProject(next);
    } catch (e) {
      console.error("Failed to load listener status:", e);
    }
  }, []);

  useEffect(() => {
    loadProjects().then((list) => loadSessions(list));
    loadStatuses();
    const timer = setInterval(async () => {
      const list = await loadProjects();
      await loadSessions(list);
      await loadStatuses();
    }, 3000);
    return () => clearInterval(timer);
  }, [loadProjects, loadSessions, loadStatuses]);

  /** 启动/停止某项目的一路监听。监听属于项目，不是全局开关。 */
  const handleToggleListener = async (
    project: Project,
    kind: ListenKind,
    active: boolean,
  ) => {
    const key = `${project.id}:${kind}`;
    setBusyKey(key);
    setBanner(null);
    try {
      if (active) {
        const current = statusesByProject[project.id]?.[kind];
        if (current) {
          await invoke("stop_listener", { id: current.id });
        }
      } else {
        const channel = new Channel<ListenerUpdate>();
        channel.onmessage = (message) => {
          if (message.type === "status") {
            setStatusesByProject((prev) => ({
              ...prev,
              [message.status.project_id]: {
                ...(prev[message.status.project_id] ?? {}),
                [message.status.kind]: message.status,
              },
            }));
          } else if (message.type === "event") {
            setRefreshToken((token) => token + 1);
          }
        };
        await invoke<string>("start_listener", {
          projectId: project.id,
          kind,
          channel,
        });
      }
      await loadStatuses();
    } catch (e) {
      setBanner(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleDeleteProject = async (id: string) => {
    if (!confirm("确定要删除这个项目吗？（已落盘的事件与会话记录不会删除）")) return;
    try {
      await invoke("delete_project", { id });
      const list = await loadProjects();
      await loadSessions(list);
      if (selectedProject?.id === id) {
        setSelectedProject(null);
        setSelectedSession(null);
      }
    } catch (e) {
      console.error("Failed to delete project:", e);
    }
  };

  const handleSelectSession = (session: Session, project: Project) => {
    setSelectedProject(project);
    setSelectedSession(session);
  };

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
        <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
          监听开关在左侧每个项目里
        </span>

        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: "8px" }}>
          <ThemeToggle />
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
            width: "300px",
            backgroundColor: "var(--bg-sidebar)",
            borderRight: "1px solid var(--border)",
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
          }}
        >
          <ProjectList
            projects={projects}
            sessionsByProject={sessionsByProject}
            statusesByProject={statusesByProject}
            selectedSession={selectedSession}
            busyKey={busyKey}
            onSelectSession={handleSelectSession}
            onEditProject={(project) => {
              setEditingProject(project);
              setShowProjectDialog(true);
            }}
            onDeleteProject={handleDeleteProject}
            onToggleListener={handleToggleListener}
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

        {selectedSession && selectedProject && (
          <ContextPanel session={selectedSession} project={selectedProject} />
        )}
      </div>

      {showSettings && (
        <SettingsPanel project={selectedProject} onClose={() => setShowSettings(false)} />
      )}

      {showCloseDialog && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            backgroundColor: "var(--overlay)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 1100,
          }}
        >
          <div
            style={{
              backgroundColor: "var(--bg-elevated)",
              border: "1px solid var(--border)",
              borderRadius: "8px",
              width: "400px",
              padding: "20px",
              boxShadow: "var(--shadow)",
            }}
          >
            <h3 style={{ margin: "0 0 8px", fontSize: "15px", color: "var(--text-primary)" }}>
              要关闭 AgentMux 吗？
            </h3>
            <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "16px" }}>
              最小化到托盘可以继续在后台接收消息与回复；选择退出会停止所有监听。
            </div>
            <div style={{ display: "flex", gap: "8px", justifyContent: "flex-end" }}>
              <button
                onClick={() => setShowCloseDialog(false)}
                style={{
                  padding: "6px 12px",
                  fontSize: "13px",
                  backgroundColor: "transparent",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                取消
              </button>
              <button
                onClick={() => {
                  setShowCloseDialog(false);
                  invoke("hide_to_tray").catch((e) => setBanner(String(e)));
                }}
                style={{
                  padding: "6px 12px",
                  fontSize: "13px",
                  backgroundColor: "var(--accent)",
                  color: "var(--accent-contrast)",
                  border: "none",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                最小化到托盘
              </button>
              <button
                onClick={() => {
                  invoke("quit_app").catch((e) => setBanner(String(e)));
                }}
                style={{
                  padding: "6px 12px",
                  fontSize: "13px",
                  backgroundColor: "var(--danger-strong)",
                  color: "#fff",
                  border: "none",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                退出
              </button>
            </div>
          </div>
        </div>
      )}

      {showProjectDialog && (
        <ProjectDialog
          project={editingProject}
          onClose={() => setShowProjectDialog(false)}
          onSaved={async () => {
            setShowProjectDialog(false);
            const list = await loadProjects();
            await loadSessions(list);
          }}
        />
      )}
    </div>
  );
}

export default App;
