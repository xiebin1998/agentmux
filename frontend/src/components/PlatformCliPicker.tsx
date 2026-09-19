import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface CliCandidate {
  path: string;
  source: string;
  /** direct=可直接启动；via_cmd=需 cmd /C；unsupported=本机无法直接启动（.ps1 等） */
  launch_mode: "direct" | "via_cmd" | "unsupported";
  version: string | null;
  auth_state: "logged_in" | "not_logged_in" | "unknown";
  detail: string | null;
}

export interface PlatformCandidates {
  platform_id: string;
  display: string;
  kind: "im" | "agent";
  candidates: CliCandidate[];
}

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

export default function PlatformCliPicker({
  kind,
  title,
  hint,
  value,
  onChange,
}: PlatformCliPickerProps) {
  const [platforms, setPlatforms] = useState<PlatformCandidates[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<PlatformCandidates[]>("list_cli_platforms", { kind });
      setPlatforms(result);

      // 默认选中第一个**能启动**的候选，用户不需要手输路径。
      const firstPlatform = result.find((p) =>
        p.candidates.some((c) => c.launch_mode !== "unsupported"),
      );
      if (firstPlatform) {
        const stillValid = value && firstPlatform.candidates.some((c) => c.path === value.path);
        if (!stillValid) {
          const launchable = firstPlatform.candidates.find(
            (c) => c.launch_mode !== "unsupported",
          )!;
          onChange({ platform: firstPlatform.platform_id, path: launchable.path });
        }
      } else {
        onChange(null);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind]);

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind]);

  const total = platforms.reduce((sum, p) => sum + p.candidates.length, 0);

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
        <button
          onClick={load}
          disabled={loading}
          style={{
            padding: "2px 8px",
            fontSize: "11px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: loading ? "not-allowed" : "pointer",
          }}
        >
          {loading ? "检测中…" : "重新检测"}
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
          maxHeight: "190px",
          overflow: "auto",
        }}
      >
        {total === 0 ? (
          <div style={{ padding: "10px 12px", color: "var(--text-muted)", fontSize: "12px" }}>
            {loading
              ? "正在从全局 PATH 与已知安装位置检测…"
              : "未检测到可用的 CLI。请先在系统里全局安装（保证 PATH 中可直接执行）后点「重新检测」。"}
          </div>
        ) : (
          platforms.map((platform) =>
            platform.candidates.length === 0 ? null : (
              <div key={platform.platform_id}>
                <div
                  style={{
                    padding: "6px 12px",
                    fontSize: "11px",
                    fontWeight: 600,
                    color: "var(--text-muted)",
                    backgroundColor: "var(--bg-sidebar)",
                    borderBottom: "1px solid var(--border)",
                  }}
                >
                  {platform.display}
                  <span style={{ fontWeight: 400, marginLeft: "6px" }}>
                    {platform.platform_id}
                  </span>
                </div>
                {platform.candidates.map((candidate) => {
                  const selected =
                    value?.platform === platform.platform_id && value?.path === candidate.path;
                  const badge = authBadge(candidate.auth_state);
                  const unusable = candidate.launch_mode === "unsupported";
                  return (
                    <label
                      key={`${platform.platform_id}:${candidate.path}`}
                      style={{
                        display: "flex",
                        alignItems: "flex-start",
                        gap: "8px",
                        padding: "8px 12px",
                        cursor: unusable ? "not-allowed" : "pointer",
                        borderBottom: "1px solid var(--border)",
                        backgroundColor: selected ? "var(--bg-active)" : "transparent",
                        opacity: unusable ? 0.6 : 1,
                      }}
                    >
                      <input
                        type="radio"
                        name={`cli-${kind}`}
                        checked={selected}
                        disabled={unusable}
                        onChange={() =>
                          onChange({ platform: platform.platform_id, path: candidate.path })
                        }
                        style={{ marginTop: "3px", accentColor: "var(--accent)" }}
                      />
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontSize: "12px",
                            color: "var(--text-primary)",
                            wordBreak: "break-all",
                            fontFamily: "ui-monospace, Consolas, monospace",
                          }}
                        >
                          {candidate.path}
                        </div>
                        <div
                          style={{
                            display: "flex",
                            flexWrap: "wrap",
                            gap: "8px",
                            marginTop: "4px",
                            fontSize: "11px",
                          }}
                        >
                          <span style={{ color: "var(--text-muted)" }}>{candidate.source}</span>
                          {candidate.version && (
                            <span style={{ color: "var(--text-secondary)" }}>
                              {candidate.version}
                            </span>
                          )}
                          {candidate.launch_mode === "unsupported" && (
                            <span style={{ color: "var(--warn)" }}>
                              本机无法直接启动（.ps1 / 脚本）
                            </span>
                          )}
                          {candidate.launch_mode === "via_cmd" && (
                            <span style={{ color: "var(--warn)" }}>包装脚本（经 cmd 启动）</span>
                          )}
                          {kind === "im" && (
                            <span style={{ color: badge.color }}>{badge.label}</span>
                          )}
                          {!candidate.version && candidate.detail && (
                            <span style={{ color: "var(--text-muted)" }}>{candidate.detail}</span>
                          )}
                        </div>
                      </div>
                    </label>
                  );
                })}
              </div>
            ),
          )
        )}
      </div>

      {error && (
        <div style={{ color: "var(--danger)", fontSize: "11px", marginTop: "4px" }}>{error}</div>
      )}
    </div>
  );
}
