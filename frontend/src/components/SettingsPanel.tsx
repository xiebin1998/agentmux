import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Project {
  id: string;
  name: string;
  work_dir: string;
}

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

/** 某个项目**实际生效**的运行期设置（由后端按项目解析后返回）。 */
interface RuntimeSettings {
  project_id: string;
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
  project: Project | null;
  onClose: () => void;
}

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

export default function SettingsPanel({ project, onClose }: SettingsPanelProps) {
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
      const [nextConfig, nextPaths] = await Promise.all([
        invoke<AppConfig>("get_config"),
        invoke<DataPaths>("data_paths"),
      ]);
      setConfig(nextConfig);
      setPaths(nextPaths);
      if (project) {
        setRuntime(
          await invoke<RuntimeSettings>("project_runtime_settings", {
            projectId: project.id,
          }),
        );
      } else {
        setRuntime(null);
      }
    } catch (e) {
      setStatus("读取设置失败：" + String(e));
    }
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project?.id]);

  const patch = (changes: Partial<AppConfig>) => {
    setConfig((current) => (current ? { ...current, ...changes } : current));
  };

  const handleSave = async () => {
    if (!config) return;
    setSaving(true);
    setStatus(null);
    try {
      await invoke("set_config", { config });
      await load();
      setStatus("已保存。Agent 启动参数、压缩策略、身份改动需重启对应项目的监听才生效。");
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
        projectId: project?.id ?? null,
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
      {project ? (
        <div style={section}>
          <div style={sectionTitle}>当前项目生效值（{project.name}）</div>
          <Row k="回复引擎" v={runtime?.reply_enabled ? "已启用" : "未启用（只记录）"} tone={runtime?.reply_enabled ? "ok" : "muted"} />
          <Row
            k="驱动 Agent 的工作目录"
            v={runtime?.agent_cwd ?? project.work_dir}
            tone="plain"
          />
          <Row
            k="实际使用的 Agent CLI"
            v={runtime?.agent_cli_path ?? "未解析到（无法生成回复）"}
            tone={runtime?.agent_cli_path ? "plain" : "bad"}
          />
          <Row k="Agent 平台" v={runtime?.agent_platform ?? "—"} />
          <Row k="生成超时" v={`${(runtime?.timeout_ms ?? 0) / 1000}s`} />
          <Row k="回复字数上限" v={`${runtime?.max_chars ?? 0} 字符`} />
          <Row
            k="群上下文"
            v={
              runtime?.context_enabled
                ? `已启用（${runtime.context_message_limit} 条 / ${runtime.context_max_chars} 字符）`
                : "已禁用"
            }
          />
          <Row k="实际使用的 IM CLI" v={runtime?.im_cli_path ?? "未解析到"} />
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "8px" }}>
            上面是**运行期真实生效值**。回复开关/超时/字数/上下文在「编辑项目」里改；
            **改完需重启该项目的监听才生效**。
          </div>
        </div>
      ) : (
        <div style={section}>
          <div style={sectionTitle}>项目相关设置</div>
          <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
            回复开关、工作目录、超时、字数上限、上下文预算、CLI 选择都是**按项目**配置的。
            请在左侧选中一个项目，或点项目上的「编辑」进行修改。
          </div>
        </div>
      )}

      <div style={section}>
        <div style={sectionTitle}>身份（全局）</div>
        <label style={label}>自身 openDingTalkId（用于跳过自己发的消息）</label>
        <input
          type="text"
          value={config.self_open_dingtalk_id ?? ""}
          onChange={(e) => patch({ self_open_dingtalk_id: e.target.value || null })}
          placeholder="留空则不跳过；可在「回复历史」里核对后一键采用"
          style={input}
        />
      </div>

      <div style={section}>
        <div style={sectionTitle}>Agent 启动参数（全局）</div>
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
          v1 只允许只读工具白名单。会话参数由系统追加在末尾，不要在这里填写。
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>自动压缩（全局）</div>
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
          触发阈值尚无实测依据，留空表示该维度不触发自动压缩，不会拍脑袋默认。
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
      </div>

      <div style={section}>
        <div style={sectionTitle}>数据与诊断</div>
        <Row k="配置文件" v={paths?.config_path ?? "—"} />
        <Row k="数据目录" v={paths?.data_dir ?? "—"} />
        <Row k="归档目录" v={paths?.archive_dir ?? "—"} />

        <div style={{ marginTop: "12px" }}>
          <label style={label}>
            导入旧版数据（指向旧工程的 data 目录）
            {project ? `，导入到项目「${project.name}」` : "，需先选中项目才能归类"}
          </label>
          <input
            type="text"
            value={legacyPath}
            onChange={(e) => setLegacyPath(e.target.value)}
            placeholder="例如：D:\workSpase\idea\dingtalk-event-host\data"
            style={input}
          />
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "4px" }}>
            只导入事件 / 回复台账 / 会话；不导入旧版日志。重复事件按 message_id 去重。
          </div>
          <button
            onClick={handleImport}
            disabled={importing || !legacyPath.trim() || !project}
            style={{
              marginTop: "8px",
              padding: "6px 14px",
              fontSize: "12px",
              backgroundColor:
                importing || !project ? "var(--text-muted)" : "var(--accent)",
              color: "var(--accent-contrast)",
              border: "none",
              borderRadius: "4px",
              cursor: importing || !project ? "not-allowed" : "pointer",
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
          {saving ? "保存中…" : "保存全局设置"}
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
      <span style={{ color: "var(--text-secondary)", flex: "0 0 150px" }}>{k}</span>
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
          width: "640px",
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
