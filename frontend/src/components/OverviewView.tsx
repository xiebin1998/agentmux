import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useProviders, type CliCandidate } from "../providers";

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
  project_id: string;
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

const mono: CSSProperties = {
  fontFamily: "ui-monospace, Consolas, monospace",
  fontSize: "11px",
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
  const [anomalies, setAnomalies] = useState<EventRow[]>([]);

  const { platforms, loading, error, checkedAt, refresh } = useProviders();

  const loadRuntime = useCallback(async () => {
    try {
      const [nextStats, nextListeners, malformed, failed] = await Promise.all([
        invoke<Stats>("get_stats"),
        invoke<ListenerStatus[]>("listener_status"),
        invoke<EventRow[]>("list_events", { limit: ANOMALY_LIMIT, malformedOnly: true }),
        invoke<EventRow[]>("list_events", { limit: ANOMALY_LIMIT, failedOnly: true }),
      ]);
      setStats(nextStats);
      setListeners(nextListeners);
      setAnomalies([...malformed, ...failed].slice(0, ANOMALY_LIMIT));
    } catch (e) {
      console.error("Failed to load overview:", e);
    }
  }, []);

  useEffect(() => {
    loadRuntime();
    const timer = setInterval(loadRuntime, 3000);
    return () => clearInterval(timer);
  }, [loadRuntime]);

  const im = platforms.filter((p) => p.kind === "im");
  const agent = platforms.filter((p) => p.kind === "agent");

  return (
    <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
      {/* 内置 CLI：IM 与 Agent 分开展示 */}
      <div style={sectionStyle}>
        <div style={{ display: "flex", alignItems: "center", marginBottom: "10px" }}>
          <span style={{ ...sectionTitle, marginBottom: 0 }}>内置 CLI</span>
          <span style={{ fontSize: "11px", color: "var(--text-muted)", marginLeft: "10px" }}>
            {loading ? "检测中…" : checkedAt ? `上次检测 ${checkedAt}` : "启动时检测一次"}
          </span>
          <button
            onClick={refresh}
            disabled={loading}
            title="重新检测"
            style={{
              marginLeft: "auto",
              display: "flex",
              alignItems: "center",
              gap: "6px",
              padding: "2px 10px",
              fontSize: "12px",
              lineHeight: 1.4,
              backgroundColor: loading ? "var(--bg-active)" : "transparent",
              color: loading ? "var(--accent)" : "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              cursor: loading ? "not-allowed" : "pointer",
            }}
          >
            <span className={loading ? "spin" : undefined}>⟳</span>
            {loading ? "检测中" : "重新检测"}
          </button>
        </div>

        {error && (
          <div style={{ fontSize: "12px", color: "var(--danger)", marginBottom: "8px" }}>
            检测失败：{error}
          </div>
        )}

        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "16px" }}>
          <div>
            <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "6px" }}>
              IM 平台
            </div>
            {im.map((platform) => (
              <CliRow key={platform.platform_id} platform={platform} showAuth />
            ))}
          </div>
          <div>
            <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "6px" }}>
              Agent 平台
            </div>
            {agent.map((platform) => (
              <CliRow key={platform.platform_id} platform={platform} />
            ))}
          </div>
        </div>
      </div>

      <div style={sectionStyle}>
        <div style={sectionTitle}>监听</div>
        {listeners.length === 0 ? (
          <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>
            当前没有监听实例。在左侧项目里启动「@我」或「单聊」。
          </div>
        ) : (
          listeners.map((listener) => (
            <div
              key={listener.id}
              style={{
                fontSize: "12px",
                marginBottom: "8px",
                display: "flex",
                gap: "10px",
                flexWrap: "wrap",
              }}
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
        <div style={sectionTitle}>事件与回复统计</div>
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
        <div style={sectionTitle}>最近异常</div>
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

function CliRow({
  platform,
  showAuth,
}: {
  platform: { platform_id: string; display: string; command: string; candidates: CliCandidate[] };
  showAuth?: boolean;
}) {
  const candidate = platform.candidates[0];
  const usable = Boolean(candidate && candidate.path !== "");
  const authColor =
    candidate?.auth_state === "logged_in"
      ? "var(--success)"
      : candidate?.auth_state === "not_logged_in"
        ? "var(--warn)"
        : "var(--text-muted)";
  const authLabel =
    candidate?.auth_state === "logged_in"
      ? "已登录"
      : candidate?.auth_state === "not_logged_in"
        ? "未登录"
        : "登录态未知";

  return (
    <div style={{ padding: "6px 0", borderBottom: "1px solid var(--border)" }}>
      <div style={{ display: "flex", gap: "8px", alignItems: "baseline", flexWrap: "wrap" }}>
        <span style={{ fontSize: "13px", color: usable ? "var(--text-primary)" : "var(--text-muted)" }}>
          {platform.display}
        </span>
        <span style={{ ...mono, color: "var(--text-secondary)" }}>{platform.command}</span>
        <span style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
          {candidate?.version ?? "版本未知"}
        </span>
        {showAuth && usable && <span style={{ fontSize: "11px", color: authColor }}>{authLabel}</span>}
        {!usable && (
          <span style={{ fontSize: "11px", color: "var(--warn)" }}>
            {candidate?.detail ?? "未检测到"}
          </span>
        )}
      </div>
      {usable && (
        <div style={{ ...mono, color: "var(--text-muted)", wordBreak: "break-all" }}>
          {candidate.path}
        </div>
      )}
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
