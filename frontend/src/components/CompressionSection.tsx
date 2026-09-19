import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Summary {
  conversation_id: string;
  content: string;
  source_events: number;
  updated_at: string;
}

interface CompressionSectionProps {
  /** 压缩要走 Agent，用哪个 CLI 与工作目录由项目决定。 */
  projectId: string;
  conversationId: string;
}

export default function CompressionSection({
  projectId,
  conversationId,
}: CompressionSectionProps) {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const result = await invoke<Summary | null>("get_summary", { conversationId });
      setSummary(result);
      setDraft(result?.content ?? "");
    } catch (e) {
      console.error("Failed to load summary:", e);
    }
  }, [conversationId]);

  useEffect(() => {
    load();
    setEditing(false);
    setMessage(null);
  }, [load]);

  // 手动压缩需二次确认（D-60）。
  const handleCompress = async () => {
    if (!confirm("将调用 Agent 压缩该会话上下文并覆盖现有摘要，确定继续？")) return;
    setBusy(true);
    setMessage(null);
    try {
      const result = await invoke<Summary>("compress_now", { projectId, conversationId });
      setSummary(result);
      setDraft(result.content);
      setMessage("压缩完成，已生效");
    } catch (e) {
      setMessage("压缩失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSave = async () => {
    setBusy(true);
    setMessage(null);
    try {
      await invoke("update_summary", { conversationId, content: draft });
      await load();
      setEditing(false);
      setMessage("摘要已保存");
    } catch (e) {
      setMessage("保存失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async () => {
    if (!confirm("删除该会话摘要？删除后回复将退回使用原始上下文。")) return;
    setBusy(true);
    try {
      await invoke("delete_summary", { conversationId });
      await load();
      setMessage("摘要已删除");
    } catch (e) {
      setMessage("删除失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const covered = summary?.source_events ?? 0;

  return (
    <div style={{ padding: "16px" }}>
      <div
        style={{
          fontSize: "11px",
          fontWeight: 600,
          color: "var(--text-secondary)",
          textTransform: "uppercase",
          marginBottom: "12px",
        }}
      >
        上下文压缩
      </div>

      <div style={{ fontSize: "12px", color: "var(--text-secondary)", marginBottom: "8px" }}>
        已覆盖 {covered} 条事件
        {summary ? ` · 更新于 ${new Date(summary.updated_at).toLocaleString()}` : " · 暂无摘要"}
      </div>

      {editing ? (
        <>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            rows={6}
            style={{
              width: "100%",
              padding: "8px",
              fontSize: "12px",
              lineHeight: 1.5,
              resize: "vertical",
            }}
          />
          <div style={{ display: "flex", gap: "8px", marginTop: "8px" }}>
            <button
              onClick={handleSave}
              disabled={busy}
              style={buttonStyle("var(--accent)")}
            >
              保存
            </button>
            <button
              onClick={() => {
                setEditing(false);
                setDraft(summary?.content ?? "");
              }}
              style={buttonStyle("transparent")}
            >
              取消
            </button>
          </div>
        </>
      ) : (
        <>
          <div
            style={{
              fontSize: "12px",
              color: summary ? "var(--text-primary)" : "var(--text-muted)",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
              maxHeight: "160px",
              overflow: "auto",
              lineHeight: 1.5,
            }}
          >
            {summary?.content ?? "尚无摘要。开启自动压缩或点击「立即压缩」生成。"}
          </div>
          <div style={{ display: "flex", gap: "8px", marginTop: "10px", flexWrap: "wrap" }}>
            <button
              onClick={handleCompress}
              disabled={busy}
              style={buttonStyle("var(--accent)")}
            >
              {busy ? "压缩中…" : "立即压缩"}
            </button>
            {summary && (
              <>
                <button onClick={() => setEditing(true)} style={buttonStyle("transparent")}>
                  编辑
                </button>
                <button
                  onClick={handleDelete}
                  disabled={busy}
                  style={buttonStyle("transparent", "var(--danger)")}
                >
                  删除
                </button>
              </>
            )}
          </div>
        </>
      )}

      {message && (
        <div style={{ fontSize: "11px", color: "var(--text-secondary)", marginTop: "8px" }}>
          {message}
        </div>
      )}
    </div>
  );
}

function buttonStyle(background: string, color = "var(--accent-contrast)"): CSSProperties {
  return {
    padding: "5px 10px",
    fontSize: "12px",
    backgroundColor: background,
    color,
    border: background === "transparent" ? "1px solid var(--border)" : "none",
    borderRadius: "4px",
    cursor: "pointer",
  };
}
