import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AppConfig {
  theme: "dark" | "light" | "system";
  im_platform: string;
  agent_platform: string;
  im_cli_path: string | null;
  agent_cli_path: string | null;
  self_open_dingtalk_id: string | null;
  agent_cwd: string | null;
  agent_args: string[] | null;
  reply_enabled: boolean;
  reply_timeout_ms: number;
  reply_max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
  auto_compress: boolean;
  compress_trigger_turns: number | null;
  compress_trigger_chars: number | null;
}

interface RuntimeSettings {
  reply_enabled: boolean;
  agent_platform: string;
  agent_cli_path: string | null;
  agent_args: string[] | null;
  agent_cwd: string;
  timeout_ms: number;
  max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
  auto_compress: boolean;
  compress_trigger_turns: number | null;
  compress_trigger_chars: number | null;
  self_open_dingtalk_id: string | null;
  im_cli_path: string | null;
}

interface DataPaths {
  config_path: string;
  data_dir: string;
  archive_dir: string;
}

interface SettingsPanelProps {
  onClose: () => void;
}

const TIMEOUT_PRESETS = [60_000, 120_000, 300_000];

const label: CSSProperties = {
  display: "block",
  color: "var(--text-secondary)",
  fontSize: "12px",
  marginBottom: "4px",
};

const input: CSSProperties = {
  width: "100%",
  padding: "6px 10px",
  fontSize: "13px",
};

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-muted)",
  textTransform: "uppercase",
  marginBottom: "10px",
};

const section: CSSProperties = {
  border: "1px solid var(--border)",
  borderRadius: "6px",
  padding: "14px",
  marginBottom: "14px",
};

export default function SettingsPanel({ onClose }: SettingsPanelProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(null);
  const [paths, setPaths] = useState<DataPaths | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [legacyPath, setLegacyPath] = useState("");
  const [importing, setImporting] = useState(false);
  const [importReport, setImportReport] = useState<string | null>(null);

  const load = async () => {
    try {
      const [nextConfig, nextRuntime, nextPaths] = await Promise.all([
        invoke<AppConfig>("get_config"),
        invoke<RuntimeSettings>("runtime_settings"),
        invoke<DataPaths>("data_paths"),
      ]);
      setConfig(nextConfig);
      setRuntime(nextRuntime);
      setPaths(nextPaths);
    } catch (e) {
      setStatus("读取设置失败：" + String(e));
    }
  };

  useEffect(() => {
    load();
  }, []);

  const patch = (changes: Partial<AppConfig>) => {
    setConfig((current) => (current ? { ...current, ...changes } : current));
  };

  const handleSave = async () => {
    if (!config) return;
    setSaving(true);
    setStatus(null);
    try {
      await invoke("set_config", { config });
      const next = await invoke<RuntimeSettings>("apply_settings");
      setRuntime(next);
      setStatus("已保存并即时生效（回复开关与预算无需重启监听）");
    } catch (e) {
      setStatus("保存失败：" + String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleImport = async () => {
    if (!confirm("从该目录导入旧版事件、回复台账与会话？重复事件会自动跳过。")) return;
    setImporting(true);
    setImportReport(null);
    try {
      const report = await invoke<Record<string, unknown>>("import_legacy", {
        path: legacyPath,
      });
      setImportReport(JSON.stringify(report, null, 2));
    } catch (e) {
      setImportReport("导入失败：" + String(e));
    } finally {
      setImporting(false);
    }
  };

  if (!config) {
    return (
      <Shell onClose={onClose}>
        <div style={{ color: "var(--text-muted)", fontSize: "13px" }}>加载中…</div>
      </Shell>
    );
  }
  return (
    <Shell onClose={onClose}>
      <div style={section}>
        <div style={sectionTitle}>回复（A5.1.1）</div>
        <label style={{ display: "flex", alignItems: "center", gap: "8px", fontSize: "13px", marginBottom: "10px" }}>
          <input
            type="checkbox"
            checked={config.reply_enabled}
            onChange={(e) => patch({ reply_enabled: e.target.checked })}
            style={{ accentColor: "var(--accent)" }}
          />
          启用自动回复（关闭时只记录，不发送任何消息）
        </label>
        <div style={{ display: "flex", gap: "12px" }}>
          <div style={{ flex: 1 }}>
            <label style={label}>生成超时（A9.1.3）</label>
            <select
              value={config.reply_timeout_ms}
              onChange={(e) => patch({ reply_timeout_ms: Number(e.target.value) })}
              style={input}
            >
              {TIMEOUT_PRESETS.map((preset) => (
                <option key={preset} value={preset}>
                  {preset / 1000}s
                </option>
              ))}
            </select>
          </div>
          <div style={{ flex: 1 }}>
            <label style={label}>回复字数上限（A9.1.4）</label>
            <input
              type="number"
              min={1}
              value={config.reply_max_chars}
              onChange={(e) => patch({ reply_max_chars: Number(e.target.value) })}
              style={input}
            />
          </div>
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>身份与工作目录</div>
        <div style={{ marginBottom: "10px" }}>
          <label style={label}>自身 openDingTalkId（A9.1.1，用于跳过自己发的消息）</label>
          <input
            type="text"
            value={config.self_open_dingtalk_id ?? ""}
            onChange={(e) => patch({ self_open_dingtalk_id: e.target.value || null })}
            placeholder="留空则不跳过；可从事件流的「发送人身份」里核对后回填"
            style={input}
          />
        </div>
        <div>
          <label style={label}>Agent 工作目录（A9.1.2，Agent 可见范围）</label>
          <input
            type="text"
            value={config.agent_cwd ?? ""}
            onChange={(e) => patch({ agent_cwd: e.target.value || null })}
            placeholder="留空 = 程序配置目录下的专用子目录"
            style={input}
          />
        </div>
        <div style={{ marginTop: "10px" }}>
          <label style={label}>Agent 启动参数（A2.2.4，空格或逗号分隔）</label>
          <input
            type="text"
            value={(config.agent_args ?? []).join(" ")}
            onChange={(e) => {
              const parts = e.target.value.split(/[\s,]+/).filter(Boolean);
              patch({ agent_args: parts.length > 0 ? parts : null });
            }}
            placeholder="留空 = 该平台的只读默认参数"
            style={input}
          />
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "4px" }}>
            v1 只允许只读工具白名单（D-20）。会话参数由系统追加在末尾，不要在
            这里填写。改参数不影响既有会话记录。
          </div>
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>群上下文（A9.1.5）</div>
        <label style={{ display: "flex", alignItems: "center", gap: "8px", fontSize: "13px", marginBottom: "10px" }}>
          <input
            type="checkbox"
            checked={config.context_enabled}
            onChange={(e) => patch({ context_enabled: e.target.checked })}
            style={{ accentColor: "var(--accent)" }}
          />
          注入最近消息作为上下文
        </label>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginBottom: "10px" }}>
          关闭后回答可能缺少上下文（D-75 尚未实测长期影响）
        </div>
        <div style={{ display: "flex", gap: "12px" }}>
          <div style={{ flex: 1 }}>
            <label style={label}>消息条数</label>
            <input
              type="number"
              min={1}
              value={config.context_message_limit}
              onChange={(e) => patch({ context_message_limit: Number(e.target.value) })}
              disabled={!config.context_enabled}
              style={input}
            />
          </div>
          <div style={{ flex: 1 }}>
            <label style={label}>字符预算</label>
            <input
              type="number"
              min={1}
              value={config.context_max_chars}
              onChange={(e) => patch({ context_max_chars: Number(e.target.value) })}
              disabled={!config.context_enabled}
              style={input}
            />
          </div>
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>自动压缩（A7.1.1 / A7.1.2）</div>
        <label style={{ display: "flex", alignItems: "center", gap: "8px", fontSize: "13px", marginBottom: "10px" }}>
          <input
            type="checkbox"
            checked={config.auto_compress}
            onChange={(e) => patch({ auto_compress: e.target.checked })}
            style={{ accentColor: "var(--accent)" }}
          />
          启用自动压缩（默认关闭）
        </label>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginBottom: "10px" }}>
          触发阈值（D-59）尚无实测依据，建议先在真实会话上记录长度与回答质量的关系再定值；
          留空表示该维度不触发自动压缩，不会拍脑袋默认。
        </div>
        <div style={{ display: "flex", gap: "12px" }}>
          <div style={{ flex: 1 }}>
            <label style={label}>按新增轮数触发（留空=不触发）</label>
            <input
              type="number"
              min={1}
              value={config.compress_trigger_turns ?? ""}
              onChange={(e) =>
                patch({ compress_trigger_turns: e.target.value ? Number(e.target.value) : null })
              }
              disabled={!config.auto_compress}
              style={input}
            />
          </div>
          <div style={{ flex: 1 }}>
            <label style={label}>按字符数触发（留空=不触发）</label>
            <input
              type="number"
              min={1}
              value={config.compress_trigger_chars ?? ""}
              onChange={(e) =>
                patch({ compress_trigger_chars: e.target.value ? Number(e.target.value) : null })
              }
              disabled={!config.auto_compress}
              style={input}
            />
          </div>
        </div>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "8px" }}>
          连续压缩失败 3 次会自动暂停自动压缩（避免反复失败拖慢回复）。
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>设置生效状态（A9.1.6 / D-76）</div>
        <Row
          k="回复引擎"
          v={runtime?.reply_enabled ? "已启用" : "未启用（只记录）"}
          tone={runtime?.reply_enabled ? "ok" : "muted"}
        />
        <Row
          k="实际使用的 Agent CLI"
          v={runtime?.agent_cli_path ?? "未解析到（无法生成回复）"}
          tone={runtime?.agent_cli_path ? "plain" : "bad"}
        />
        <Row k="Agent 平台" v={runtime?.agent_platform ?? "—"} />
        <Row
          k="实际启动参数"
          v={
            runtime?.agent_args && runtime.agent_args.length > 0
              ? runtime.agent_args.join(" ")
              : "（平台只读默认值）"
          }
        />
        <Row k="实际工作目录" v={runtime?.agent_cwd ?? "—"} />
        <Row
          k="自动压缩"
          v={
            runtime?.auto_compress
              ? runtime.compress_trigger_turns || runtime.compress_trigger_chars
                ? "已启用（阈值已配置）"
                : "已启用但未配阈值，不会自动触发"
              : "未启用"
          }
          tone={runtime?.auto_compress ? "plain" : "muted"}
        />
        <Row k="实际使用的 IM CLI" v={runtime?.im_cli_path ?? "未解析到"} />
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "8px" }}>
          上面这组是**运行期真实生效值**，不是 settings.json 的回显。回复开关、超时、字数、上下文预算保存后即时生效；
          Agent / IM CLI 路径变更后需重新启动对应监听。
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>数据与诊断（A9.2.1 / A9.2.2）</div>
        <Row k="配置文件" v={paths?.config_path ?? "—"} />
        <Row k="数据目录" v={paths?.data_dir ?? "—"} />
        <Row k="归档目录" v={paths?.archive_dir ?? "—"} />

        <div style={{ marginTop: "12px" }}>
          <label style={label}>导入旧版数据（指向旧工程的数据目录）</label>
          <input
            type="text"
            value={legacyPath}
            onChange={(e) => setLegacyPath(e.target.value)}
            placeholder="例如：D:\workSpase\idea\dingtalk-event-host\data"
            style={input}
          />
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "4px" }}>
            只导入事件 / 回复台账 / 会话；不导入旧版日志（D-78）。重复事件按 message_id 去重。
          </div>
          <button
            onClick={handleImport}
            disabled={importing || !legacyPath.trim()}
            style={{
              marginTop: "8px",
              padding: "6px 14px",
              fontSize: "12px",
              backgroundColor: importing ? "var(--text-muted)" : "var(--accent)",
              color: "var(--accent-contrast)",
              border: "none",
              borderRadius: "4px",
              cursor: importing ? "not-allowed" : "pointer",
            }}
          >
            {importing ? "导入中…" : "开始导入"}
          </button>
          {importReport && (
            <pre
              style={{
                marginTop: "10px",
                padding: "10px",
                backgroundColor: "var(--bg-app)",
                border: "1px solid var(--border)",
                borderRadius: "4px",
                fontSize: "11px",
                color: "var(--text-secondary)",
                whiteSpace: "pre-wrap",
                maxHeight: "200px",
                overflow: "auto",
              }}
            >
              {importReport}
            </pre>
          )}
        </div>
      </div>

      {status && (
        <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "10px" }}>
          {status}
        </div>
      )}

      <div style={{ display: "flex", justifyContent: "flex-end", gap: "8px" }}>
        <button
          onClick={onClose}
          style={{
            padding: "8px 16px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
            fontSize: "13px",
          }}
        >
          关闭
        </button>
        <button
          onClick={handleSave}
          disabled={saving}
          style={{
            padding: "8px 16px",
            backgroundColor: saving ? "var(--text-muted)" : "var(--accent)",
            color: "var(--accent-contrast)",
            border: "none",
            borderRadius: "4px",
            cursor: saving ? "not-allowed" : "pointer",
            fontSize: "13px",
          }}
        >
          {saving ? "保存中…" : "保存并即时生效"}
        </button>
      </div>
    </Shell>
  );
}

function Row({ k, v, tone = "plain" }: { k: string; v: string; tone?: "plain" | "ok" | "bad" | "muted" }) {
  const color =
    tone === "ok"
      ? "var(--success)"
      : tone === "bad"
        ? "var(--danger)"
        : tone === "muted"
          ? "var(--text-muted)"
          : "var(--text-primary)";

  return (
    <div style={{ display: "flex", gap: "12px", fontSize: "12px", marginBottom: "6px" }}>
      <span style={{ color: "var(--text-secondary)", flex: "0 0 130px" }}>{k}</span>
      <span
        style={{
          color,
          wordBreak: "break-all",
          fontFamily: "ui-monospace, Consolas, monospace",
        }}
      >
        {v}
      </span>
    </div>
  );
}

function Shell({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        backgroundColor: "var(--overlay)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
    >
      <div
        style={{
          backgroundColor: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          borderRadius: "8px",
          width: "620px",
          maxHeight: "88vh",
          overflow: "auto",
          boxShadow: "var(--shadow)",
        }}
      >
        <div
          style={{
            padding: "16px 20px",
            borderBottom: "1px solid var(--border)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            position: "sticky",
            top: 0,
            backgroundColor: "var(--bg-elevated)",
          }}
        >
          <h2 style={{ margin: 0, color: "var(--text-primary)", fontSize: "16px" }}>设置</h2>
          <button
            onClick={onClose}
            style={{
              padding: "4px 8px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "none",
              cursor: "pointer",
              fontSize: "18px",
            }}
          >
            ×
          </button>
        </div>
        <div style={{ padding: "20px" }}>{children}</div>
      </div>
    </div>
  );
}
