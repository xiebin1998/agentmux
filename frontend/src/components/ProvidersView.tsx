import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

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
  agent_platform: string;
  im_cli_path: string | null;
}

/** A2.1.3：未安装与未登录必须一眼可辨，不能合并成一个状态。 */
function installBadge(candidate: CliCandidate | undefined) {
  if (!candidate) return { label: "未检测到", color: "var(--danger)" };
  return { label: "已安装", color: "var(--success)" };
}

function authBadge(state: CliCandidate["auth_state"] | undefined) {
  switch (state) {
    case "logged_in":
      return { label: "已登录", color: "var(--success)" };
    case "not_logged_in":
      return { label: "未登录", color: "var(--warn)" };
    default:
      return { label: "登录态未知", color: "var(--text-muted)" };
  }
}

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-muted)",
  textTransform: "uppercase",
  margin: "4px 0 10px",
};

export default function ProvidersView() {
  const [platforms, setPlatforms] = useState<PlatformCandidates[]>([]);
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [list, settings] = await Promise.all([
        invoke<PlatformCandidates[]>("list_cli_platforms"),
        invoke<RuntimeSettings>("runtime_settings"),
      ]);
      setPlatforms(list);
      setRuntime(settings);
      setCheckedAt(new Date().toLocaleString());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const im = platforms.filter((p) => p.kind === "im");
  const agent = platforms.filter((p) => p.kind === "agent");

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
        }}
      >
        <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-primary)" }}>
          提供方检测
        </span>
        <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
          检测为显式动作，结果不缓存{checkedAt ? ` · 本次检测于 ${checkedAt}` : ""}
        </span>
        <button
          onClick={load}
          disabled={loading}
          style={{
            marginLeft: "auto",
            padding: "4px 12px",
            fontSize: "12px",
            backgroundColor: loading ? "var(--text-muted)" : "var(--accent)",
            color: "var(--accent-contrast)",
            border: "none",
            borderRadius: "4px",
            cursor: loading ? "not-allowed" : "pointer",
          }}
        >
          {loading ? "检测中…" : "重新检测全部"}
        </button>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
        {error && (
          <div style={{ color: "var(--danger)", fontSize: "12px", marginBottom: "12px" }}>
            检测失败：{error}
          </div>
        )}

        <div style={sectionTitle}>IM 提供方</div>
        {im.map((platform) => (
          <PlatformCard
            key={platform.platform_id}
            platform={platform}
            selected={platform.candidates.some((c) => c.path === runtime?.im_cli_path)}
            expanded={expanded === platform.platform_id}
            onToggle={() =>
              setExpanded(expanded === platform.platform_id ? null : platform.platform_id)
            }
            onRefresh={load}
          />
        ))}

        <div style={{ ...sectionTitle, marginTop: "20px" }}>Agent 提供方</div>
        {agent.map((platform) => (
          <PlatformCard
            key={platform.platform_id}
            platform={platform}
            selected={platform.platform_id === runtime?.agent_platform}
            expanded={expanded === platform.platform_id}
            onToggle={() =>
              setExpanded(expanded === platform.platform_id ? null : platform.platform_id)
            }
            onRefresh={load}
          />
        ))}
      </div>
    </div>
  );
}

function PlatformCard({
  platform,
  selected,
  expanded,
  onToggle,
  onRefresh,
}: {
  platform: PlatformCandidates;
  selected: boolean;
  expanded: boolean;
  onToggle: () => void;
  onRefresh: () => void;
}) {
  const best = platform.candidates[0];
  const install = installBadge(best);
  const auth = authBadge(best?.auth_state);

  return (
    <div
      style={{
        border: `1px solid ${selected ? "var(--accent)" : "var(--border)"}`,
        borderRadius: "6px",
        padding: "12px",
        marginBottom: "10px",
        backgroundColor: "var(--bg-elevated)",
      }}
    >
      <div style={{ display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: "13px", fontWeight: 600, color: "var(--text-primary)" }}>
          {platform.display}
        </span>
        <span
          style={{
            fontFamily: "ui-monospace, Consolas, monospace",
            fontSize: "11px",
            color: "var(--text-muted)",
          }}
        >
          {platform.platform_id}
        </span>
        <span style={{ fontSize: "11px", color: install.color }}>{install.label}</span>
        {platform.kind === "im" && (
          <span style={{ fontSize: "11px", color: auth.color }}>{auth.label}</span>
        )}
        {selected && (
          <span
            style={{
              fontSize: "11px",
              padding: "1px 6px",
              borderRadius: "8px",
              backgroundColor: "var(--accent)",
              color: "var(--accent-contrast)",
            }}
          >
            当前选用
          </span>
        )}
        <div style={{ marginLeft: "auto", display: "flex", gap: "6px" }}>
          <button
            onClick={onRefresh}
            style={{
              padding: "2px 8px",
              fontSize: "11px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "3px",
              cursor: "pointer",
            }}
          >
            重新检测
          </button>
          <button
            onClick={onToggle}
            style={{
              padding: "2px 8px",
              fontSize: "11px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "3px",
              cursor: "pointer",
            }}
          >
            {expanded ? "收起诊断" : "查看诊断"}
          </button>
        </div>
      </div>

      {platform.candidates.length === 0 ? (
        <div style={{ fontSize: "12px", color: "var(--danger)", marginTop: "8px" }}>
          未在全局 PATH 与已知安装位置找到可执行文件。请先全局安装后重新检测。
        </div>
      ) : (
        <div style={{ marginTop: "8px" }}>
          <div
            style={{
              fontFamily: "ui-monospace, Consolas, monospace",
              fontSize: "11px",
              color: "var(--text-primary)",
              wordBreak: "break-all",
            }}
          >
            {best.path}
          </div>
          <div style={{ fontSize: "11px", color: "var(--text-muted)", marginTop: "4px" }}>
            版本：{best.version ?? "未知（不显示推测值）"} · 来源：{best.source} · 候选{" "}
            {platform.candidates.length} 个
            {best.is_wrapper && (
              <span style={{ color: "var(--warn)" }}>
                {" "}
                · 首选是包装脚本，无法直接启动
              </span>
            )}
          </div>
          {best.detail && (
            <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginTop: "4px" }}>
              {best.detail}
            </div>
          )}
        </div>
      )}

      {expanded && (
        <pre
          style={{
            marginTop: "10px",
            padding: "10px",
            backgroundColor: "var(--bg-app)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            fontFamily: "ui-monospace, Consolas, monospace",
            fontSize: "11px",
            color: "var(--text-secondary)",
            overflow: "auto",
            maxHeight: "220px",
            whiteSpace: "pre-wrap",
          }}
        >
          {JSON.stringify(platform.candidates, null, 2)}
        </pre>
      )}
    </div>
  );
}
