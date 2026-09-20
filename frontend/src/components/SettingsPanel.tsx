import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

interface Project {
  id: string;
  name: string;
  work_dir: string;
  context_max_chars: number;
}

interface AppConfig {
  theme: "dark" | "light" | "system";
  im_platform: string;
  agent_platform: string;
  im_cli_path: string | null;
  agent_cli_path: string | null;
  agent_cwd: string | null;
  agent_args: string[] | null;
  /** 权限放行档位："" / "auto" / "no_ask" / "full"；空 = 用 CLI 默认 */
  permission_mode?: string | null;
  reply_enabled: boolean;
  reply_timeout_ms: number;
  reply_max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
  auto_compress: boolean;
  compress_trigger_percent?: number | null;
  compress_trigger_turns: number | null;
  compress_trigger_chars: number | null;
}

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
  compress_trigger_percent: number | null;
  compress_trigger_chars: number | null;
  self_open_id: string | null;
  im_cli_path: string | null;
}

interface DataPaths {
  app_root: string;
  config_path: string;
  data_dir: string;
  archive_dir: string;
  is_default: boolean;
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

const DEFAULT_PERCENT = 80;

export default function SettingsPanel({ project, onClose }: SettingsPanelProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(null);
  const [paths, setPaths] = useState<DataPaths | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [moving, setMoving] = useState(false);

  const load = async () => {
    try {
      const [nextConfig, nextPaths] = await Promise.all([
        invoke<AppConfig>("get_config"),
        invoke<DataPaths>("data_paths"),
      ]);
      setConfig(nextConfig);
      setPaths(nextPaths);
      setRuntime(
        project
          ? await invoke<RuntimeSettings>("project_runtime_settings", { projectId: project.id })
          : null,
      );
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
      setStatus("已保存。Agent 启动参数与压缩策略需重启对应项目的监听才生效。");
    } catch (e) {
      setStatus("保存失败：" + String(e));
    } finally {
      setSaving(false);
    }
  };

  /** 切换数据目录：只记录位置，搬迁在下次启动时做。 */
  const handleMoveDataDir = async () => {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择数据目录（配置文件、数据库、归档都会放在这里）",
        defaultPath: paths?.data_dir,
      });
      if (typeof picked !== "string" || !picked) return;
      if (
        !confirm(
          `把数据目录改为：\n${picked}\n\n重启后生效：现有数据会复制过去，旧目录不会被删除。确定吗？`,
        )
      ) {
        return;
      }
      setMoving(true);
      const next = await invoke<DataPaths>("set_data_dir", { path: picked });
      setPaths(next);
      setStatus("数据目录已记录，重启 AgentMux 后生效（现有数据会一并复制过去）。");
    } catch (e) {
      setStatus("切换数据目录失败：" + String(e));
    } finally {
      setMoving(false);
    }
  };

  const handleResetDataDir = async () => {
    if (!confirm("恢复默认数据目录？重启后生效。")) return;
    try {
      const next = await invoke<DataPaths>("set_data_dir", { path: "" });
      setPaths(next);
      setStatus("已恢复默认数据目录，重启后生效。");
    } catch (e) {
      setStatus("恢复失败：" + String(e));
    }
  };

  if (!config) {
    return (
      <Shell onClose={onClose}>
        <div style={{ color: "var(--text-muted)", fontSize: "13px" }}>加载中…</div>
      </Shell>
    );
  }

  const percent = config.compress_trigger_percent ?? DEFAULT_PERCENT;
  const budget = runtime?.context_max_chars ?? project?.context_max_chars ?? config.context_max_chars;
  const approxChars = Math.max(1, Math.round((budget * percent) / 100));

  return (
    <Shell onClose={onClose}>
      {project ? (
        <div style={section}>
          <div style={sectionTitle}>当前项目生效值（{project.name}）</div>
          <Row
            k="回复引擎"
            v={runtime?.reply_enabled ? "已启用" : "未启用（只记录）"}
            tone={runtime?.reply_enabled ? "ok" : "muted"}
          />
          <Row k="驱动 Agent 的工作目录" v={runtime?.agent_cwd ?? project.work_dir} />
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
          <Row k="自身身份" v={runtime?.self_open_id ?? "未设置（会回复自己发的消息）"} />
          <Row k="实际使用的 IM CLI" v={runtime?.im_cli_path ?? "未解析到"} />
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "8px" }}>
            回复开关、工作目录、超时、字数、上下文预算在项目的「编辑」里改；
            <strong>改完需重启该项目的监听才生效</strong>。自身身份用于跳过自己发的消息，
            由配置文件提供，界面不提供修改入口。
          </div>
        </div>
      ) : (
        <div style={section}>
          <div style={sectionTitle}>项目相关设置</div>
          <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
            回复开关、工作目录、超时、字数上限、上下文预算、CLI 选择都是<strong>按项目</strong>配置的。
            请在左侧选中一个项目，或点项目上的「编辑」进行修改。
          </div>
        </div>
      )}

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
        <label
          style={{ display: "flex", alignItems: "center", gap: "8px", fontSize: "13px", marginBottom: "12px" }}
        >
          <input
            type="checkbox"
            checked={config.auto_compress}
            onChange={(e) => patch({ auto_compress: e.target.checked })}
            style={{ accentColor: "var(--accent)" }}
          />
          启用自动压缩（压缩后会换新会话，摘要承接前情）
        </label>

        <div style={{ opacity: config.auto_compress ? 1 : 0.5 }}>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "baseline",
              marginBottom: "6px",
            }}
          >
            <label style={label}>触发阈值：上下文用到 {percent}%</label>
            <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
              ≈ {approxChars} 字符
            </span>
          </div>
          <input
            type="range"
            className="agentmux-range"
            min={0}
            max={100}
            step={5}
            value={percent}
            disabled={!config.auto_compress}
            onChange={(e) => patch({ compress_trigger_percent: Number(e.target.value) })}
          />
          <div style={{ display: "flex", justifyContent: "space-between", fontSize: "10px", color: "var(--text-muted)" }}>
            <span>0%</span>
            <span>50%</span>
            <span>100%</span>
          </div>
          <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "6px" }}>
            按各项目的上下文预算换算成字符数；设为 0% 表示不自动压缩。
            连续压缩失败 3 次会自动暂停。
          </div>
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>权限放行（全局）</div>
        <select
          value={config.permission_mode ?? ""}
          onChange={(e) => patch({ permission_mode: e.target.value || null })}
          style={input}
        >
          <option value="">跟随 CLI 默认（推荐）</option>
          <option value="auto">自动批准：由 CLI 判断该不该放行</option>
          <option value="no_ask">不询问：不弹审批提示</option>
          <option value="full">完全放行：跳过所有检查</option>
        </select>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "4px" }}>
          决定 Agent 执行命令、联网这类操作时要不要人工放行。三个 CLI 的写法不一样
          （Qoder / Claude 用 <code>--permission-mode</code>，Codex 用 <code>-a</code>），
          系统会按当前平台自动翻译，你不用记。
          {config.permission_mode === "full" && (
            <>
              <br />
              <strong>完全放行意味着不再有人审查 Agent 要执行什么</strong>：机器人会按
              任何人在钉钉里发来的消息行动，风险明显变大。只在你清楚后果时用，
              用到就尽快切回来。
            </>
          )}
          {config.permission_mode === "auto" && (
            <>
              <br />
              注意：这一档要过安全分类器，分类器不可用时相关操作会被整片拦下
              （日志里会写「分类器暂时不可用」）—— 那时可临时切到「完全放行」。
            </>
          )}
        </div>
      </div>

      <div style={section}>
        <div style={sectionTitle}>数据位置</div>
        <Row k="数据目录" v={paths?.data_dir ?? "—"} tone={paths?.is_default ? "plain" : "ok"} />
        <Row k="配置文件" v={paths?.config_path ?? "—"} />
        <Row k="归档目录" v={paths?.archive_dir ?? "—"} />
        <Row k="定位文件" v={paths?.app_root ?? "—"} />
        <div style={{ display: "flex", gap: "8px", marginTop: "10px" }}>
          <button
            onClick={handleMoveDataDir}
            disabled={moving}
            style={{
              padding: "6px 14px",
              fontSize: "12px",
              backgroundColor: moving ? "var(--text-muted)" : "var(--accent)",
              color: "var(--accent-contrast)",
              border: "none",
              borderRadius: "4px",
              cursor: moving ? "not-allowed" : "pointer",
            }}
          >
            {moving ? "处理中…" : "更改数据目录…"}
          </button>
          {paths && !paths.is_default && (
            <button
              onClick={handleResetDataDir}
              style={{
                padding: "6px 14px",
                fontSize: "12px",
                backgroundColor: "transparent",
                color: "var(--text-secondary)",
                border: "1px solid var(--border)",
                borderRadius: "4px",
                cursor: "pointer",
              }}
            >
              恢复默认
            </button>
          )}
        </div>
        <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "8px" }}>
          配置文件、数据库与归档都放在这个目录下。<strong>安装时选的自定义位置不会被自动采用</strong>
          —— 装到 `Program Files` 后写入需要管理员权限，所以默认仍放在用户目录；
          这里改完<strong>重启生效</strong>，现有数据会复制过去（旧目录不会删）。
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
        style={{ color, wordBreak: "break-all", fontFamily: "ui-monospace, Consolas, monospace" }}
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
        overflow: "hidden",
      }}
    >
      {/* 三段式：只有中间那块滚。整块弹窗滚的话，标题和 × 会被表单里的下拉顶出可视区。 */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          backgroundColor: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          borderRadius: "8px",
          width: "660px",
          maxHeight: "88vh",
          overflow: "hidden",
          boxShadow: "var(--shadow)",
        }}
      >
        <div
          style={{
            flexShrink: 0,
            padding: "16px 20px",
            borderBottom: "1px solid var(--border)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
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
        <div style={{ flex: 1, minHeight: 0, overflow: "auto", overscrollBehavior: "contain", padding: "20px" }}>
          {children}
        </div>
      </div>
    </div>
  );
}
