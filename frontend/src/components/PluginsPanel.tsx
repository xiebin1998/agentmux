import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PluginInfo {
  id: string;
  name: string;
  kind: string;
  source: "builtin";
  enabled: boolean;
  risk: "low" | "medium" | "high";
  declared_capabilities: string[];
  detect: string;
  detail: string | null;
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
  const [message, setMessage] = useState<string | null>(null);

  const load = async () => {
    try {
      setPlugins(await invoke<PluginInfo[]>("list_plugins"));
    } catch (e) {
      setMessage("读取适配器失败：" + String(e));
    }
  };

  useEffect(() => {
    load();
  }, []);

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
          <h2 style={{ margin: 0, color: "var(--text-primary)", fontSize: "16px" }}>
            AI 适配器
          </h2>
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

        <div style={{ padding: "20px" }}>
          <div style={{ ...card, backgroundColor: "var(--bg-sidebar)" }}>
            <div style={sectionTitle}>内置适配器</div>
            <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
              这些适配器用 Rust 实现、随宿主一起编译，在宿主进程内运行，
              因此不引入额外进程风险（均为低风险）。停用后不会出现在项目可选的平台里。
            </div>
          </div>

          {plugins.map((plugin) => {
            const risk = RISK[plugin.risk] ?? RISK.low;
            return (
              <div key={plugin.id} style={card}>
                <div
                  style={{ display: "flex", alignItems: "center", gap: "8px", marginBottom: "6px" }}
                >
                  <span style={{ fontSize: "13px", color: "var(--text-primary)", fontWeight: 600 }}>
                    {plugin.name}
                  </span>
                  <span style={{ ...mono, color: "var(--text-muted)" }}>{plugin.id}</span>
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
                      onChange={() => toggle(plugin)}
                      style={{ accentColor: "var(--accent)" }}
                    />
                    {plugin.enabled ? "已启用" : "已停用"}
                  </label>
                </div>

                <div
                  style={{
                    fontSize: "11px",
                    color: "var(--text-muted)",
                    display: "flex",
                    gap: "10px",
                    flexWrap: "wrap",
                  }}
                >
                  <span>kind: {plugin.kind}</span>
                  <span>来源: 内置</span>
                  <span>声明能力: {plugin.declared_capabilities.join(", ")}</span>
                </div>

                {plugin.detail && (
                  <div style={{ fontSize: "11px", color: "var(--text-muted)", marginTop: "6px" }}>
                    {plugin.detail}
                  </div>
                )}
              </div>
            );
          })}

          <div style={{ ...card, borderStyle: "dashed" }}>
            <div style={sectionTitle}>外部插件</div>
            <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>
              本版**暂不提供**外部插件（manifest + NDJSON over stdio）的安装与扫描。
              后续需要时再接入。
            </div>
          </div>

          {message && (
            <div style={{ fontSize: "12px", color: "var(--text-secondary)" }}>{message}</div>
          )}
        </div>
      </div>
    </div>
  );
}
