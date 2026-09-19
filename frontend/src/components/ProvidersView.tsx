import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
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
  /** platform_id → 自身在该平台上的身份 id（跳过自己发的消息用） */
  const [identities, setIdentities] = useState<Record<string, string>>({});
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [savingId, setSavingId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const loadIdentities = async () => {
    try {
      const config = await invoke<{
        im_identities?: Record<string, string>;
        self_open_dingtalk_id?: string | null;
      }>("get_config");
      const map: Record<string, string> = { ...(config.im_identities ?? {}) };
      // 旧版本只有一个全局钉钉身份，读出来当作钉钉平台的值。
      if (!map.dingtalk && config.self_open_dingtalk_id) {
        map.dingtalk = config.self_open_dingtalk_id;
      }
      setIdentities(map);
      setDraft(map);
    } catch (e) {
      console.error("Failed to load identities:", e);
    }
  };

  useEffect(() => {
    loadIdentities();
  }, []);

  const saveIdentity = async (platformId: string) => {
    setSavingId(platformId);
    setNotice(null);
    try {
      await invoke("set_im_identity", {
        platformId,
        identity: draft[platformId] ?? "",
      });
      await loadIdentities();
      setNotice(`${platformId} 的自身身份已保存（下次启动该项目监听时生效）`);
    } catch (e) {
      setNotice("保存失败：" + String(e));
    } finally {
      setSavingId(null);
    }
  };

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
            backgroundColor: loading ? "var(--bg-active)" : "transparent",
            color: loading ? "var(--accent)" : "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: loading ? "not-allowed" : "pointer",
          }}
        >
          <span className={loading ? "spin" : undefined}>⟳</span>
        </button>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "16px" }}>
        {(error || notice) && (
          <div
            style={{
              color: error ? "var(--danger)" : "var(--text-secondary)",
              fontSize: "12px",
              marginBottom: "12px",
            }}
          >
            {error ? `检测失败：${error}` : notice}
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
            identity={draft[platform.platform_id] ?? ""}
            savedIdentity={identities[platform.platform_id] ?? ""}
            saving={savingId === platform.platform_id}
            onIdentityChange={(value) =>
              setDraft((current) => ({ ...current, [platform.platform_id]: value }))
            }
            onIdentitySave={() => saveIdentity(platform.platform_id)}
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
  identity,
  savedIdentity,
  saving,
  onIdentityChange,
  onIdentitySave,
}: {
  platform: PlatformCandidates;
  expanded: boolean;
  onToggle: () => void;
  identity?: string;
  savedIdentity?: string;
  saving?: boolean;
  onIdentityChange?: (value: string) => void;
  onIdentitySave?: () => void;
}) {
  const best = platform.candidates[0];
  const install = installBadge(best);
  const auth = authBadge(best?.auth_state);
  const dirty = identity !== undefined && identity !== (savedIdentity ?? "");

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

      {/* 身份按 IM 平台设置：以后接入别的 IM，各自有自己的身份 */}
      {platform.kind === "im" && onIdentityChange && (
        <div
          style={{
            marginTop: "10px",
            paddingTop: "10px",
            borderTop: "1px solid var(--border)",
          }}
        >
          <label
            style={{
              display: "block",
              fontSize: "11px",
              color: "var(--text-secondary)",
              marginBottom: "4px",
            }}
          >
            自身身份 id（你在该平台上的身份，用于跳过自己发的消息）
          </label>
          <div style={{ display: "flex", gap: "6px" }}>
            <input
              type="text"
              value={identity ?? ""}
              onChange={(e) => onIdentityChange(e.target.value)}
              placeholder="留空 = 不跳过（会回复自己发的消息）"
              style={{
                flex: 1,
                minWidth: 0,
                padding: "4px 8px",
                fontSize: "12px",
                fontFamily: "ui-monospace, Consolas, monospace",
              }}
            />
            <button
              onClick={onIdentitySave}
              disabled={!dirty || saving}
              style={{
                padding: "4px 12px",
                fontSize: "12px",
                backgroundColor: dirty && !saving ? "var(--accent)" : "var(--text-muted)",
                color: "var(--accent-contrast)",
                border: "none",
                borderRadius: "4px",
                cursor: dirty && !saving ? "pointer" : "not-allowed",
              }}
            >
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
          <div style={{ fontSize: "10px", color: "var(--text-muted)", marginTop: "4px" }}>
            身份按平台独立存放，换 IM 平台互不影响。可在「回复历史」里核对发送身份后点「采用该身份」。
          </div>
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
