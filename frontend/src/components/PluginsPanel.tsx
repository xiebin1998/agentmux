import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PluginInfo {
  id: string;
  name: string;
  version: string | null;
  kind: string;
  source: "builtin" | "external";
  enabled: boolean;
  risk: "low" | "medium" | "high";
  declared_capabilities: string[];
  protocol: number;
  entry: string | null;
  path: string | null;
  detect: string;
  detail: string | null;
}

interface PluginEnv {
  root: string;
  manifest_example: string;
}

interface PluginsPanelProps {
  onClose: () => void;
}

const RISK: Record<string, { label: string; color: string }> = {
  low: { label: "低风险", color: "var(--success)" },
  medium: { label: "中风险", color: "var(--warn)" },
  high: { label: "高风险", color: "var(--danger)" },
};

const sectionTitle: CSSProperties = {
  fontSize: "11px",
  fontWeight: 600,
  color: "var(--text-muted)",
  textTransform: "uppercase",
  marginBottom: "10px",
};

const card: CSSProperties = {
  border: "1px solid var(--border)",
  borderRadius: "6px",
  padding: "12px",
  marginBottom: "10px",
};

const mono: CSSProperties = {
  fontFamily: "ui-monospace, Consolas, monospace",
  fontSize: "11px",
  wordBreak: "break-all",
};

export default function PluginsPanel({ onClose }: PluginsPanelProps) {
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [env, setEnv] = useState<PluginEnv | null>(null);
  const [doc, setDoc] = useState<string | null>(null);
  const [showDoc, setShowDoc] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const loadBuiltin = async () => {
    try {
      const [list, environment] = await Promise.all([
        invoke<PluginInfo[]>("list_plugins"),
        invoke<PluginEnv>("plugin_env"),
      ]);
      setPlugins(list);
      setEnv(environment);
    } catch (e) {
      setMessage("读取插件失败：" + String(e));
    }
  };

  useEffect(() => {
    loadBuiltin();
  }, []);

  const handleScan = async () => {
    setScanning(true);
    setMessage(null);
    try {
      const list = await invoke<PluginInfo[]>("scan_plugins");
      setPlugins(list);
      const external = list.filter((p) => p.source === "external");
      setMessage(
        external.length === 0
          ? "未发现外部插件（把插件目录放到上面的根目录后重新扫描）"
          : `已扫描 ${external.length} 个外部插件`,
      );
    } catch (e) {
      setMessage("扫描失败：" + String(e));
    } finally {
      setScanning(false);
    }
  };

  const toggle = async (plugin: PluginInfo) => {
    try {
      await invoke("set_plugin_enabled", { id: plugin.id, enabled: !plugin.enabled });
      setPlugins((current) =>
        current.map((p) => (p.id === plugin.id ? { ...p, enabled: !p.enabled } : p)),
      );
    } catch (e) {
      setMessage("切换失败：" + String(e));
    }
  };

  const toggleDoc = async () => {
    if (!doc) {
      try {
        setDoc(await invoke<string>("plugin_protocol_doc"));
      } catch (e) {
        setMessage("读取协议文档失败：" + String(e));
        return;
      }
    }
    setShowDoc((current) => !current);
  };

  const builtin = plugins.filter((p) => p.source === "builtin");
  const external = plugins.filter((p) => p.source === "external");

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
          width: "680px",
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
          <h2 style={{ margin: 0, color: "var(--text-primary)", fontSize: "16px" }}>插件</h2>
          <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
            <button
              onClick={handleScan}
              disabled={scanning}
              style={{
                padding: "4px 10px",
                fontSize: "12px",
                backgroundColor: "var(--accent)",
                color: "var(--accent-contrast)",
                border: "none",
                borderRadius: "4px",
                cursor: scanning ? "not-allowed" : "pointer",
              }}
            >
              {scanning ? "扫描中…" : "扫描外部插件"}
            </button>
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
        </div>

        <div style={{ padding: "20px" }}>
          {env && (
            <div style={{ ...card, backgroundColor: "var(--bg-sidebar)" }}>
              <div style={sectionTitle}>插件根目录（A8.2.2，v1 只支持一个）</div>
              <div style={{ ...mono, color: "var(--text-primary)", marginBottom: "8px" }}>
                {env.root}
              </div>
              <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "4px" }}>
                manifest.json 示例：
              </div>
              <pre
                style={{
                  ...mono,
                  margin: 0,
                  padding: "8px",
                  backgroundColor: "var(--bg-app)",
                  borderRadius: "4px",
                  maxHeight: "140px",
                  overflow: "auto",
                  color: "var(--text-secondary)",
                }}
              >
                {env.manifest_example}
              </pre>
            </div>
          )}

          <div style={{ marginBottom: "18px" }}>
            <div style={sectionTitle}>内置插件（A8.1.2）</div>
            {builtin.map((plugin) => (
              <PluginCard key={plugin.id} plugin={plugin} onToggle={toggle} />
            ))}
          </div>

          <div style={{ marginBottom: "18px" }}>
            <div style={sectionTitle}>外部插件（A8.1.2 / A8.2.3）</div>
            {external.length === 0 ? (
              <div style={{ fontSize: "12px", color: "var(--text-muted)" }}>
                未扫描到外部插件。点右上角「扫描外部插件」。
              </div>
            ) : (
              external.map((plugin) => (
                <PluginCard key={plugin.id} plugin={plugin} onToggle={toggle} />
              ))
            )}
          </div>

          <div style={{ marginBottom: "12px" }}>
            <button
              onClick={toggleDoc}
              style={{
                padding: "6px 12px",
                fontSize: "12px",
                backgroundColor: "transparent",
                color: "var(--text-secondary)",
                border: "1px solid var(--border)",
                borderRadius: "4px",
                cursor: "pointer",
              }}
            >
              {showDoc ? "收起协议文档（A8.2.1）" : "查看插件协议文档（A8.2.1）"}
            </button>
            {showDoc && doc && (
              <pre
                style={{
                  ...mono,
                  marginTop: "10px",
                  padding: "12px",
                  backgroundColor: "var(--bg-app)",
                  border: "1px solid var(--border)",
                  borderRadius: "6px",
                  maxHeight: "320px",
                  overflow: "auto",
                  whiteSpace: "pre-wrap",
                  color: "var(--text-secondary)",
                }}
              >
                {doc}
              </pre>
            )}
          </div>

          {message && (
            <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>{message}</div>
          )}
        </div>
      </div>
    </div>
  );
}

function PluginCard({
  plugin,
  onToggle,
}: {
  plugin: PluginInfo;
  onToggle: (plugin: PluginInfo) => void;
}) {
  const risk = RISK[plugin.risk] ?? RISK.low;
  const detectOk = plugin.detect === "ready";

  return (
    <div style={card}>
      <div style={{ display: "flex", alignItems: "center", gap: "8px", marginBottom: "6px" }}>
        <span style={{ fontSize: "13px", color: "var(--text-primary)", fontWeight: 600 }}>
          {plugin.name}
        </span>
        <span style={{ ...mono, color: "var(--text-muted)" }}>{plugin.id}</span>
        {plugin.version && (
          <span style={{ fontSize: "11px", color: "var(--text-secondary)" }}>{plugin.version}</span>
        )}
        <span style={{ fontSize: "11px", color: risk.color }}>{risk.label}</span>
        <label
          style={{
            marginLeft: "auto",
            fontSize: "12px",
            color: "var(--text-secondary)",
            display: "flex",
            alignItems: "center",
            gap: "6px",
          }}
        >
          <input
            type="checkbox"
            checked={plugin.enabled}
            onChange={() => onToggle(plugin)}
            style={{ accentColor: "var(--accent)" }}
          />
          {plugin.enabled ? "已启用" : "已停用"}
        </label>
      </div>

      <div style={{ fontSize: "11px", color: "var(--text-muted)", display: "flex", gap: "10px", flexWrap: "wrap" }}>
        <span>kind: {plugin.kind}</span>
        <span>protocol: {plugin.protocol}</span>
        <span>来源: {plugin.source === "builtin" ? "内置" : "外部"}</span>
        <span style={{ color: detectOk ? "var(--success)" : "var(--danger)" }}>
          检测: {detectOk ? "就绪" : "未通过"}
        </span>
      </div>

      {plugin.declared_capabilities.length > 0 && (
        <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginTop: "6px" }}>
          声明能力: {plugin.declared_capabilities.join(", ")}
        </div>
      )}

      {plugin.entry && (
        <div style={{ ...mono, color: "var(--text-secondary)", marginTop: "6px" }}>
          入口: {plugin.entry}
        </div>
      )}
      {plugin.path && (
        <div style={{ ...mono, color: "var(--text-muted)", marginTop: "2px" }}>
          目录: {plugin.path}
        </div>
      )}
      {plugin.detail && (
        <div style={{ fontSize: "11px", color: "var(--text-muted)", marginTop: "6px" }}>
          {plugin.detail}
        </div>
      )}
    </div>
  );
}
