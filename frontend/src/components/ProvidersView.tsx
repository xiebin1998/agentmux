import { useState, type CSSProperties } from "react";
import { useProviders, type CliCandidate, type PlatformCandidates } from "../providers";

/** 未安装与未登录必须一眼可辨，不能合并成一个状态。 */
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
  const { platforms, loading, error, checkedAt, refresh } = useProviders();
  const [expanded, setExpanded] = useState<string | null>(null);

  const im = platforms.filter((p) => p.kind === "im");
  const agent = platforms.filter((p) => p.kind === "agent");

  const toggle = (platformId: string) =>
    setExpanded(expanded === platformId ? null : platformId);

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
          启动时检测一次
          {checkedAt ? ` · 上次检测 ${checkedAt}` : ""}
        </span>
        <button
          onClick={refresh}
          disabled={loading}
          title="重新检测"
          style={{
            marginLeft: "auto",
            padding: "2px 10px",
            fontSize: "14px",
            lineHeight: 1.2,
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: loading ? "not-allowed" : "pointer",
          }}
        >
          {loading ? "…" : "⟳"}
        </button>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
        {error && (
          <div style={{ color: "var(--danger)", fontSize: "12px", marginBottom: "12px" }}>
            检测失败：{error}
          </div>
        )}

        {/* IM 与 Agent 分开展示 */}
        <div style={sectionTitle}>IM 提供方</div>
        {im.map((platform) => (
          <PlatformCard
            key={platform.platform_id}
            platform={platform}
            expanded={expanded === platform.platform_id}
            onToggle={() => toggle(platform.platform_id)}
          />
        ))}

        <div style={{ ...sectionTitle, marginTop: "20px" }}>Agent 提供方</div>
        {agent.map((platform) => (
          <PlatformCard
            key={platform.platform_id}
            platform={platform}
            expanded={expanded === platform.platform_id}
            onToggle={() => toggle(platform.platform_id)}
          />
        ))}
      </div>
    </div>
  );
}

function PlatformCard({
  platform,
  expanded,
  onToggle,
}: {
  platform: PlatformCandidates;
  expanded: boolean;
  onToggle: () => void;
}) {
  const best = platform.candidates[0];
  const install = installBadge(best);
  const auth = authBadge(best?.auth_state);

  return (
    <div
      style={{
        border: "1px solid var(--border)",
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
        <button
          onClick={onToggle}
          style={{
            marginLeft: "auto",
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

      {!best ? (
        <div style={{ fontSize: "12px", color: "var(--danger)", marginTop: "8px" }}>
          未在 PATH 与已知安装位置找到可执行文件。请先安装后点右上角 ⟳ 重新检测。
        </div>
      ) : (
        <div style={{ marginTop: "8px" }}>
          <div style={{ fontSize: "12px", color: "var(--text-primary)" }}>
            命令 <span style={{ fontFamily: "ui-monospace, Consolas, monospace" }}>{best.name}</span>
            {" · 版本 "}
            {best.version ?? "未知（不显示推测值）"}
          </div>
          <div
            style={{
              fontFamily: "ui-monospace, Consolas, monospace",
              fontSize: "11px",
              color: "var(--text-muted)",
              marginTop: "4px",
              wordBreak: "break-all",
            }}
          >
            启动文件：{best.path || "未解析到"}
          </div>
          {best.launch_mode === "via_cmd" && (
            <div style={{ fontSize: "11px", color: "var(--warn)", marginTop: "4px" }}>
              该命令是包装脚本，启动时经 cmd 转发
            </div>
          )}
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
