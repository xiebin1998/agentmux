import { useEffect, useState, type CSSProperties } from "react";
import { useProviders, type CliCandidate } from "../providers";

export interface CliSelection {
  platform: string;
  path: string;
}

interface PlatformCliPickerProps {
  kind: "im" | "agent";
  title: string;
  hint?: string;
  value: CliSelection | null;
  onChange: (selection: CliSelection | null) => void;
}

function authBadge(state: CliCandidate["auth_state"]) {
  switch (state) {
    case "logged_in":
      return { label: "已登录", color: "var(--success)" };
    case "not_logged_in":
      return { label: "已安装未登录", color: "var(--warn)" };
    default:
      return { label: "登录态未知", color: "var(--text-muted)" };
  }
}

const refreshButton: CSSProperties = {
  padding: "2px 8px",
  fontSize: "11px",
  backgroundColor: "transparent",
  color: "var(--text-secondary)",
  border: "1px solid var(--border)",
  borderRadius: "4px",
  cursor: "pointer",
};

/** 单选即选中，用户不需要手输路径。检测结果来自全局共享的检测上下文。 */
export default function PlatformCliPicker({
  kind,
  title,
  hint,
  value,
  onChange,
}: PlatformCliPickerProps) {
  const { platforms: all, loading, refresh } = useProviders();
  const platforms = all.filter((p) => p.kind === kind);
  const [initialized, setInitialized] = useState(false);

  // 默认选中第一个「能真正启动」的平台（只用默认值初始化一次，不覆盖用户选择）。
  useEffect(() => {
    if (initialized || platforms.length === 0) return;
    const usable = platforms.find((p) => p.candidates[0] && p.candidates[0].path !== "");
    if (usable && !value) {
      onChange({ platform: usable.platform_id, path: usable.candidates[0].path });
    }
    setInitialized(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platforms, initialized]);

  const usableCount = platforms.filter(
    (p) => p.candidates[0] && p.candidates[0].path !== "",
  ).length;

  return (
    <div style={{ marginBottom: "16px" }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: "6px",
        }}
      >
        <label style={{ color: "var(--text-secondary)", fontSize: "13px" }}>{title}</label>
        <button onClick={refresh} disabled={loading} style={refreshButton} title="重新检测">
          {loading ? "检测中…" : "⟳ 重新检测"}
        </button>
      </div>

      {hint && (
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginBottom: "6px" }}>
          {hint}
        </div>
      )}

      <div
        role="radiogroup"
        aria-label={title}
        style={{
          border: "1px solid var(--border)",
          borderRadius: "6px",
          backgroundColor: "var(--bg-input)",
          maxHeight: "210px",
          overflow: "auto",
        }}
      >
        {usableCount === 0 ? (
          <div style={{ padding: "10px 12px", color: "var(--text-muted)", fontSize: "12px" }}>
            {loading
              ? "正在按命令名检测（等价于在终端里敲 `命令 --version`）…"
              : "未检测到可用 CLI。请在终端里确认对应命令能直接执行（如 `dws version`），然后点「⟳ 重新检测」。"}
          </div>
        ) : (
          platforms.map((platform) => {
            const candidate = platform.candidates[0];
            const usable = Boolean(candidate && candidate.path !== "");
            const selected = value?.platform === platform.platform_id;
            const badge = authBadge(candidate?.auth_state ?? "unknown");

            return (
              <label
                key={platform.platform_id}
                style={{
                  display: "flex",
                  alignItems: "flex-start",
                  gap: "8px",
                  padding: "8px 12px",
                  cursor: usable ? "pointer" : "not-allowed",
                  borderBottom: "1px solid var(--border)",
                  backgroundColor: selected ? "var(--bg-active)" : "transparent",
                  opacity: usable ? 1 : 0.55,
                }}
              >
                <input
                  type="radio"
                  name={`cli-${kind}`}
                  checked={selected}
                  disabled={!usable}
                  onChange={() =>
                    onChange({ platform: platform.platform_id, path: candidate.path })
                  }
                  style={{ marginTop: "3px", accentColor: "var(--accent)" }}
                />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{ display: "flex", gap: "8px", alignItems: "baseline", flexWrap: "wrap" }}
                  >
                    <span style={{ fontSize: "13px", color: "var(--text-primary)" }}>
                      {platform.display}
                    </span>
                    <span
                      style={{
                        fontFamily: "ui-monospace, Consolas, monospace",
                        fontSize: "11px",
                        color: "var(--text-secondary)",
                      }}
                    >
                      {platform.command}
                    </span>
                    {candidate?.version && (
                      <span style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
                        {candidate.version}
                      </span>
                    )}
                  </div>
                  <div
                    style={{
                      display: "flex",
                      flexWrap: "wrap",
                      gap: "8px",
                      marginTop: "3px",
                      fontSize: "11px",
                    }}
                  >
                    {kind === "im" && usable && (
                      <span style={{ color: badge.color }}>{badge.label}</span>
                    )}
                    {!usable && (
                      <span style={{ color: "var(--warn)" }}>
                        {candidate?.detail ?? "未检测到"}
                      </span>
                    )}
                    {usable && (
                      <span
                        style={{
                          color: "var(--text-muted)",
                          fontFamily: "ui-monospace, Consolas, monospace",
                          wordBreak: "break-all",
                        }}
                      >
                        {candidate.path}
                      </span>
                    )}
                  </div>
                </div>
              </label>
            );
          })
        )}
      </div>
    </div>
  );
}
