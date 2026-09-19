import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Stats {
  total_events: number;
  malformed_events: number;
  processed_events: number;
  replied_events: number;
  failed_replies: number;
  conversations: number;
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

interface CliCandidate {
  path: string;
  source: string;
  is_wrapper: boolean;
  version: string | null;
  auth_state: "logged_in" | "not_logged_in" | "unknown";
  detail: string | null;
}

interface PlatformCandidates {
  platform_id: string;
  display: string;
  kind: "im" | "agent";
  candidates: CliCandidate[];
}

interface RuntimeSettings {
  reply_enabled: boolean;
  agent_platform: string;
  agent_cli_path: string | null;
  im_cli_path: string | null;
}

interface EventRow {
  message_id: string;
  received_at: string;
  malformed: boolean;
  reply_status: string | null;
  sender: string;
}

const ANOMALY_LIMIT = 200;

const sectionStyle: CSSProperties = {
  border: "1px solid var(--border)",
  borderRadius: "6px",
  padding: "14px",
  marginBottom: "14px",
  backgroundColor: "var(--bg-elevated)",
};

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-muted)",
  textTransform: "uppercase",
  marginBottom: "10px",
};

function stateColor(status: ListenerStatus | undefined) {
  if (!status) return "var(--text-muted)";
  if (status.ready) return "var(--success)";
  if (status.state === "abandoned") return "var(--danger)";
  if (status.state === "starting" || status.state === "backing_off") return "var(--warn)";
  return "var(--text-muted)";
}

function stateText(status: ListenerStatus | undefined) {
  if (!status) return "未启动";
  if (status.ready) return "监听中";
  return (
    {
      stopped: "已停止",
      starting: "启动中",
      running: "运行中",
      backing_off: "退避重试中",
      abandoned: "已放弃",
    }[status.state] ?? status.state
  );
}

export default function OverviewView() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [listeners, setListeners] = useState<ListenerStatus[]>([]);
  const [platforms, setPlatforms] = useState<PlatformCandidates[]>([]);
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(null);
  const [anomalies, setAnomalies] = useState<EventRow[]>([]);
  const [rechecking, setRechecking] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  /** 便宜的运行时信息，可以轮询。 */
  const loadRuntime = useCallback(async () => {
    try {
      const [nextStats, nextListeners, nextRuntime, malformed, failed] = await Promise.all([
        invoke<Stats>("get_stats"),
        invoke<ListenerStatus[]>("listener_status"),
        invoke<RuntimeSettings>("runtime_settings"),
        invoke<EventRow[]>("list_events", { limit: ANOMALY_LIMIT, malformedOnly: true }),
        invoke<EventRow[]>("list_events", { limit: ANOMALY_LIMIT, failedOnly: true }),
      ]);
      setStats(nextStats);
      setListeners(nextListeners);
      setRuntime(nextRuntime);
      // A1.1.4：异常保留上限 200 条，与归档同源。
      setAnomalies([...malformed, ...failed].slice(0, ANOMALY_LIMIT));
    } catch (e) {
      console.error("Failed to load overview:", e);
    }
  }, []);

  /**
   * 提供方检测会真的 spawn CLI 进程，**不能轮询**：
   * D-06 要求「检测为显式动作、结果不缓存」，而且每次 spawn 在 Windows 上
   * 都可能有控制台窗口开销。只在进入页面与点「一键重新检测」时跑。
   */
  const loadProviders = useCallback(async () => {
    try {
      const [list, settings] = await Promise.all([
        invoke<PlatformCandidates[]>("list_cli_platforms"),
        invoke<RuntimeSettings>("runtime_settings"),
      ]);
      setPlatforms(list);
      setRuntime(settings);
    } catch (e) {
      console.error("Failed to detect providers:", e);
    }
  }, []);

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  useEffect(() => {
    loadRuntime();
    const timer = setInterval(loadRuntime, 3000);
    return () => clearInterval(timer);
  }, [loadRuntime]);

  /** A1.1.3：串行重检全部提供方，不并发起进程。 */
  const handleRecheckAll = async () => {
    setRechecking(true);
    setMessage(null);
    try {
      await loadProviders();
      await invoke<string | null>("reset_im_cli");
      const list = await invoke<PlatformCandidates[]>("list_cli_platforms");
      setPlatforms(list);
      const im = list.find((p) => p.kind === "im");
      const missing = list.filter((p) => p.candidates.length === 0).length;
      setMessage(
        `已重检 ${list.length} 个提供方；${im?.candidates.length ?? 0} 个 IM 候选，${
          missing > 0 ? `${missing} 个未检测到` : "全部已检测到"
        }`,
      );
    } catch (e) {
      setMessage("重检失败：" + String(e));
    } finally {
      setRechecking(false);
    }
  };

  const imPlatform = platforms.find((p) => p.kind === "im");
  const agentPlatform = platforms.find((p) => p.platform_id === runtime?.agent_platform);

  return (
    <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
      <div style={sectionStyle}>
        <div style={sectionTitle}>运行总览（A1.1.1）</div>
        <div style={{ display: "flex", gap: "20px", flexWrap: "wrap" }}>
          <Item
            label="IM 提供方"
            value={
              imPlatform
                ? `${imPlatform.display} · ${imPlatform.candidates.length} 个候选${
                    imPlatform.candidates[0]?.auth_state === "logged_in"
                      ? " · 已登录"
                      : imPlatform.candidates[0]?.auth_state === "not_logged_in"
                        ? " · 未登录"
                        : ""
                  }`
                : "未检测到"
            }
            ok={Boolean(imPlatform?.candidates.length)}
          />
          <Item
            label="Agent 提供方"
            value={
              runtime?.agent_cli_path
                ? `${runtime.agent_platform} · ${agentPlatform?.candidates.length ?? 0} 个候选`
                : "未解析到可用 CLI"
            }
            ok={Boolean(runtime?.agent_cli_path)}
          />
          <Item
            label="自动回复"
            value={runtime?.reply_enabled ? "已启用" : "未启用（只记录）"}
            ok={Boolean(runtime?.reply_enabled)}
            neutral={!runtime?.reply_enabled}
          />
        </div>
      </div>

      <div style={sectionStyle}>
        <div style={sectionTitle}>监听</div>
        {listeners.length === 0 ? (
          <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>
            当前没有监听实例。在顶部工具栏启动「@我」或「单聊」。
          </div>
        ) : (
          listeners.map((listener) => (
            <div
              key={listener.id}
              style={{ fontSize: "12px", marginBottom: "8px", display: "flex", gap: "10px", flexWrap: "wrap" }}
            >
              <span style={{ color: "var(--text-secondary)", minWidth: "70px" }}>
                {listener.kind}
              </span>
              <span style={{ color: stateColor(listener) }}>{stateText(listener)}</span>
              <span style={{ color: "var(--text-muted)" }}>
                {listener.subscribe_id ?? "无订阅 id"} · bus_pid {listener.bus_pid ?? "—"} · 第{" "}
                {listener.attempts} 次
              </span>
              {listener.last_error && (
                <span style={{ color: "var(--danger)" }}>{listener.last_error}</span>
              )}
            </div>
          ))
        )}
      </div>

      <div style={sectionStyle}>
        <div style={sectionTitle}>事件与回复统计（A1.1.2）</div>
        <div style={{ display: "flex", gap: "24px", flexWrap: "wrap" }}>
          <Metric label="累计事件" value={stats?.total_events ?? 0} />
          <Metric label="已处理" value={stats?.processed_events ?? 0} />
          <Metric label="已回复" value={stats?.replied_events ?? 0} tone="ok" />
          <Metric label="回复失败" value={stats?.failed_replies ?? 0} tone="bad" />
          <Metric label="畸形事件" value={stats?.malformed_events ?? 0} tone="warn" />
          <Metric label="会话数" value={stats?.conversations ?? 0} />
        </div>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "10px" }}>
          统计口径含全部已落盘事件；回复结果单独计数，不并入已处理。
        </div>
      </div>

      <div style={sectionStyle}>
        <div style={sectionTitle}>提供方重检（A1.1.3）</div>
        <button
          onClick={handleRecheckAll}
          disabled={rechecking}
          style={{
            padding: "6px 14px",
            fontSize: "12px",
            backgroundColor: rechecking ? "var(--text-muted)" : "var(--accent)",
            color: "var(--accent-contrast)",
            border: "none",
            borderRadius: "4px",
            cursor: rechecking ? "not-allowed" : "pointer",
          }}
        >
          {rechecking ? "重检中…" : "一键重新检测全部提供方"}
        </button>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "6px" }}>
          串行重检，同一时刻最多 1 个检测进程；检测结果不缓存。
        </div>
        {message && (
          <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginTop: "8px" }}>
            {message}
          </div>
        )}
      </div>

      <div style={sectionStyle}>
        <div style={sectionTitle}>最近异常（A1.1.4）</div>
        {anomalies.length === 0 ? (
          <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>暂无异常事件</div>
        ) : (
          <>
            <div style={{ color: "var(--text-muted)", fontSize: "11px", marginBottom: "8px" }}>
              上限 {ANOMALY_LIMIT} 条，与归档同源
            </div>
            {anomalies.map((event, index) => (
              <div
                key={`${event.message_id}-${index}`}
                style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "4px" }}
              >
                <span style={{ color: event.malformed ? "var(--warn)" : "var(--danger)" }}>
                  {event.malformed ? "畸形" : "回复失败"}
                </span>{" "}
                {new Date(event.received_at).toLocaleString()} · {event.sender || "(未知发送人)"}
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function Item({
  label,
  value,
  ok,
  neutral,
}: {
  label: string;
  value: string;
  ok?: boolean;
  neutral?: boolean;
}) {
  const color = neutral ? "var(--text-muted)" : ok ? "var(--success)" : "var(--danger)";
  return (
    <div>
      <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "4px" }}>
        {label}
      </div>
      <div style={{ fontSize: "13px", color, fontWeight: 600 }}>{value}</div>
    </div>
  );
}

function Metric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "ok" | "bad" | "warn";
}) {
  const color =
    tone === "ok"
      ? "var(--success)"
      : tone === "bad"
        ? value > 0
          ? "var(--danger)"
          : "var(--text-primary)"
        : tone === "warn"
          ? value > 0
            ? "var(--warn)"
            : "var(--text-primary)"
          : "var(--text-primary)";
  return (
    <div>
      <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "2px" }}>
        {label}
      </div>
      <div style={{ fontSize: "20px", color, fontWeight: 600 }}>{value}</div>
    </div>
  );
}
