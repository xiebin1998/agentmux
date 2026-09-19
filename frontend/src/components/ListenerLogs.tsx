import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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

const STATE_TEXT: Record<ListenerState, string> = {
  stopped: "已停止",
  starting: "启动中",
  running: "监听中",
  backing_off: "退避重试中",
  abandoned: "已放弃",
};

const STATE_COLOR: Record<ListenerState, string> = {
  stopped: "var(--text-muted)",
  starting: "var(--warn)",
  running: "var(--success)",
  backing_off: "var(--warn)",
  abandoned: "var(--danger)",
};

const BACKOFF = "5s / 10s / 20s / 40s / 60s（连续失败 5 次后放弃）";

export default function ListenerLogs() {
  const [logs, setLogs] = useState<string[]>([]);
  const [statuses, setStatuses] = useState<ListenerStatus[]>([]);
  const [autoScroll, setAutoScroll] = useState(true);
  const bottomRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      const [nextLogs, nextStatuses] = await Promise.all([
        invoke<string[]>("listener_logs", { limit: 1000 }),
        invoke<ListenerStatus[]>("listener_status"),
      ]);
      setLogs(nextLogs);
      setStatuses(nextStatuses);
    } catch (e) {
      console.error("Failed to load listener logs:", e);
    }
  }, []);

  useEffect(() => {
    load();
    const timer = setInterval(load, 2000);
    return () => clearInterval(timer);
  }, [load]);

  useEffect(() => {
    if (autoScroll) bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [logs.length, autoScroll]);

  const handleClear = async () => {
    if (!confirm("清空监听日志缓冲？（已落盘的事件不受影响）")) return;
    try {
      await invoke("clear_listener_logs");
      await load();
    } catch (e) {
      console.error(e);
    }
  };

  const droppedTotal = statuses.reduce((sum, s) => sum + s.dropped_before_ready, 0);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div
        style={{
          padding: "10px 16px",
          borderBottom: "1px solid var(--border)",
          backgroundColor: "var(--bg-sidebar)",
          display: "flex",
          gap: "10px",
          alignItems: "center",
          flexWrap: "wrap",
        }}
      >
        <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-primary)" }}>
          监听日志
        </span>
        {statuses.length === 0 && (
          <span style={{ fontSize: "12px", color: "var(--text-muted)" }}>当前没有监听实例</span>
        )}
        {statuses.map((status) => (
          <span
            key={status.id}
            style={{
              fontSize: "11px",
              padding: "2px 8px",
              border: "1px solid var(--border)",
              borderRadius: "10px",
              color: STATE_COLOR[status.state],
            }}
            title={status.last_error ?? undefined}
          >
            {status.kind}: {STATE_TEXT[status.state]}
            {status.subscribe_id ? ` · ${status.subscribe_id.slice(0, 12)}…` : ""}
          </span>
        ))}
        <label
          style={{ marginLeft: "auto", fontSize: "12px", color: "var(--text-secondary)" }}
        >
          <input
            type="checkbox"
            checked={autoScroll}
            onChange={(e) => setAutoScroll(e.target.checked)}
            style={{ marginRight: "4px", accentColor: "var(--accent)" }}
          />
          自动滚到底
        </label>
        <button
          onClick={handleClear}
          style={{
            padding: "4px 10px",
            fontSize: "12px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
          }}
        >
          清空
        </button>
      </div>

      {/* A3.2.3：恢复期可能丢消息，必须明示而不是装作没发生 */}
      {(droppedTotal > 0 || statuses.some((s) => s.state === "backing_off")) && (
        <div
          style={{
            padding: "6px 16px",
            backgroundColor: "var(--warn)",
            color: "#000",
            fontSize: "12px",
          }}
        >
          监听曾中断/重试：就绪前丢弃 {droppedTotal} 条，退避序列 {BACKOFF}。
          中断窗口内的消息钉钉侧不缓存，无法补回。
        </div>
      )}

      <div
        style={{
          flex: 1,
          overflow: "auto",
          padding: "10px 16px",
          fontFamily: "ui-monospace, Consolas, monospace",
          fontSize: "12px",
          lineHeight: 1.6,
          backgroundColor: "var(--bg-app)",
        }}
      >
        {logs.length === 0 ? (
          <div style={{ color: "var(--text-muted)" }}>
            暂无日志。启动监听后，dws 的 stderr / 运行事件会实时出现在这里。
          </div>
        ) : (
          logs.map((line, index) => (
            <div
              key={index}
              style={{
                color: line.includes("失败") || line.includes("错误")
                  ? "var(--danger)"
                  : line.includes("ready")
                    ? "var(--success)"
                    : "var(--text-secondary)",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {line}
            </div>
          ))
        )}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
