import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import ProjectList from "./components/ProjectList";
import MessageView from "./components/MessageView";
import ContextPanel from "./components/ContextPanel";
import ProjectDialog from "./components/ProjectDialog";
import EventLog from "./components/EventLog";
import ListenerLogs from "./components/ListenerLogs";
import OverviewView from "./components/OverviewView";
import SettingsPanel from "./components/SettingsPanel";
import { ThemeToggle } from "./theme";
import type { Project, Session } from "./types";

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
  /** 群名或对方用户名；还没拉到元信息时为空 */
  name: string;
  /** group / direct / unknown */
  kind: string;
}

type ListenerUpdate =
  | { type: "status"; status: ListenerStatus }
  | { type: "event"; event: unknown }
  | { type: "log"; listener_id: string; line: string };

/** check_update 命令的返回。 */
interface UpdateInfo {
  current_version: string;
  version: string;
  notes: string | null;
  date: string | null;
}

/** 更新提示条的状态机。 */
type UpdateProgress =
  | { phase: "idle" }
  | { phase: "downloading"; percent: number }
  | { phase: "installing" }
  | { phase: "failed"; message: string };

type View = "overview" | "events" | "logs";

const VIEWS: { view: View; label: string }[] = [
  { view: "overview", label: "运行总览" },
  { view: "events", label: "事件与回复" },
  { view: "logs", label: "监听日志" },
];

function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [sessionsByProject, setSessionsByProject] = useState<Record<string, Session[]>>({});
  /** 没有项目归属的历史会话（升级前的数据），仍要能看到并能归入项目 */
  const [unassignedSessions, setUnassignedSessions] = useState<Session[]>([]);
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
  /** 当前运行的版本号；拿不到就显示不出来，不影响其它功能。 */
  const [appVersion, setAppVersion] = useState<string | null>(null);
  /** 远端可用的新版本；null = 没有更新（离线、被墙、已是最新都算）。 */
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress>({ phase: "idle" });
  /** 启动只检查一次更新：StrictMode 下 effect 会跑两遍。 */
  const updateChecked = useRef(false);

  // 点 × 时后端拦下关闭并通知前端，由用户选「最小化到托盘」还是「退出」。
  useEffect(() => {
    const pending = listen("close-requested", () => setShowCloseDialog(true));
    return () => {
      pending.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  // 启动只检查一次更新。失败就当没有更新：离线、被墙、还没配更新源都是常态，
  // 不该为此弹错误（原因会写进「监听日志」）。
  useEffect(() => {
    if (updateChecked.current) return;
    updateChecked.current = true;

    getVersion()
      .then(setAppVersion)
      .catch(() => {});

    invoke<UpdateInfo | null>("check_update")
      .then((info) => {
        if (info) setUpdate(info);
      })
      .catch(() => {});
  }, []);

  // 下载进度与「开始安装」由后端推事件过来。
  useEffect(() => {
    const progress = listen<number>("update-progress", (event) => {
      setUpdateProgress({ phase: "downloading", percent: event.payload });
    });
    const installing = listen("update-installing", () => {
      setUpdateProgress({ phase: "installing" });
    });
    return () => {
      progress.then((unlisten) => unlisten()).catch(() => {});
      installing.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  const handleInstallUpdate = async () => {
    setUpdateProgress({ phase: "downloading", percent: 0 });
    try {
      await invoke("install_update");
      // Windows 上安装器启动后本进程会被结束，正常不会走到这里。
      setUpdateProgress({ phase: "failed", message: "安装没有启动，请手动下载安装包" });
    } catch (e) {
      setUpdateProgress({ phase: "failed", message: String(e) });
    }
  };

  const updateText = (() => {
    if (!update) return "";
    switch (updateProgress.phase) {
      case "downloading":
        return `正在下载 v${update.version} · ${updateProgress.percent}%`;
      case "installing":
        return `正在安装 v${update.version}，安装器会自动重启应用`;
      case "failed":
        return `发现 v${update.version} · 更新失败：${updateProgress.message}`;
      default:
        return `发现新版本 v${update.version}（当前 v${update.current_version}）`;
    }
  })();
  const canInstallUpdate =
    updateProgress.phase === "idle" || updateProgress.phase === "failed";

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
    const toSession = (projectId: string, conversation: ConversationSummary): Session => ({
      id: conversation.conversation_id,
      project_id: projectId,
      // 命名规范：群聊用群名、单聊用对方用户名；两者都是 dws 给的 name。
      // 还没拉到（或 dws 说 nameKnown=false）时回退成会话 id，不至于显示空白。
      name: conversation.name?.trim() ? conversation.name : conversation.conversation_id,
      conversation_id: conversation.conversation_id,
      created_at: conversation.last_received_at,
      kind: conversation.kind || "unknown",
      name_known: Boolean(conversation.name?.trim()),
    });

    await Promise.all(
      list.map(async (project) => {
        try {
          const conversations = await invoke<ConversationSummary[]>("list_conversations", {
            projectId: project.id,
          });
          next[project.id] = conversations.map((c) => toSession(project.id, c));
        } catch (e) {
          console.error("Failed to load conversations for", project.id, e);
          next[project.id] = [];
        }
      }),
    );
    setSessionsByProject(next);

    // 升级前的历史会话没有 project_id，不属于任何项目，但必须可见
    try {
      const orphans = await invoke<ConversationSummary[]>("list_conversations", {
        unassigned: true,
      });
      setUnassignedSessions(orphans.map((c) => toSession("", c)));
    } catch (e) {
      console.error("Failed to load unassigned conversations:", e);
      setUnassignedSessions([]);
    }
  }, []);

  /** 把无归属的历史会话归入某个项目。 */
  const handleAssignConversation = async (conversationId: string, projectId: string) => {
    if (!projectId) return;
    try {
      await invoke("assign_conversation", { conversationId, projectId });
      const list = await loadProjects();
      await loadSessions(list);
      setBanner(null);
    } catch (e) {
      setBanner(String(e));
    }
  };

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
    // 会话名/群聊单聊来自 dws，会真的启子进程，所以只在启动时拉一次，
    // 之后靠会话窗口里的「刷新会话信息」手动触发（与 CLI 检测同一原则）。
    const refreshMetaOnce = async () => {
      try {
        await invoke("refresh_conversation_meta");
      } catch (e) {
        console.error("Failed to refresh conversation meta:", e);
      }
    };
    refreshMetaOnce().then(() => loadProjects().then((list) => loadSessions(list)));
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

  /** 删除会话：从左树与统计里去掉（事件仍在「事件与回复」里可查）。 */
  const handleDeleteConversation = async (session: Session) => {
    if (
      !confirm(
        `删除会话「${session.name}」？\n\n` +
          "它会从左树与统计里消失，Agent 会话记录一并清掉；" +
          "历史事件不会被删（仍可在「事件与回复」里查）。\n" +
          "之后该会话再来新消息，它会自动重新出现。",
      )
    ) {
      return;
    }
    try {
      await invoke("delete_conversation", { conversationId: session.conversation_id });
      const list = await loadProjects();
      await loadSessions(list);
      if (selectedSession?.conversation_id === session.conversation_id) {
        setSelectedSession(null);
      }
    } catch (e) {
      setBanner(String(e));
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

  const handleSelectSession = (session: Session, project: Project | null) => {
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
        {appVersion && (
          <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>v{appVersion}</span>
        )}
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

      {update && (
        <div
          style={{
            padding: "6px 16px",
            backgroundColor: "var(--accent)",
            color: "var(--accent-contrast)",
            fontSize: "12px",
            display: "flex",
            alignItems: "center",
            gap: "10px",
            flexWrap: "wrap",
          }}
        >
          <span>{updateText}</span>
          {canInstallUpdate && (
            <button
              onClick={handleInstallUpdate}
              style={{
                padding: "3px 10px",
                fontSize: "12px",
                backgroundColor: "transparent",
                color: "var(--accent-contrast)",
                border: "1px solid var(--accent-contrast)",
                borderRadius: "4px",
                cursor: "pointer",
              }}
            >
              立即更新
            </button>
          )}
        </div>
      )}

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
            unassignedSessions={unassignedSessions}
            statusesByProject={statusesByProject}
            selectedSession={selectedSession}
            busyKey={busyKey}
            onSelectSession={handleSelectSession}
            onAssignConversation={handleAssignConversation}
            onDeleteSession={handleDeleteConversation}
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
              <OverviewView projects={projects} />
            ) : view === "events" ? (
              <EventLog refreshToken={refreshToken} projects={projects} />
            ) : (
              <ListenerLogs projects={projects} />
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
