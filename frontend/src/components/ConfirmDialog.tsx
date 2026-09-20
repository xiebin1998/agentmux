import type { ReactNode } from "react";

interface ConfirmDialogProps {
  title: string;
  /** 说清会发生什么：删掉哪些东西、为什么不可恢复。 */
  body: ReactNode;
  confirmText: string;
  /** 危险操作：确定按钮用红色。 */
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * 危险操作的二次确认。
 *
 * 不用浏览器的 `confirm()`：它是同步阻塞的，样式与主题对不上，而且一旦被宿主
 * 静默拦掉就会「不问就删」。这里用应用内弹窗，跑得掉也看得见。
 */
export default function ConfirmDialog({
  title,
  body,
  confirmText,
  busy,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        backgroundColor: "var(--overlay)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1200,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          backgroundColor: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          borderRadius: "8px",
          width: "420px",
          maxWidth: "92vw",
          maxHeight: "80vh",
          overflow: "hidden",
          boxShadow: "var(--shadow)",
        }}
      >
        <div
          style={{
            flexShrink: 0,
            padding: "14px 20px",
            borderBottom: "1px solid var(--border)",
            color: "var(--text-primary)",
            fontSize: "15px",
            fontWeight: 600,
          }}
        >
          {title}
        </div>

        <div
          style={{
            flex: 1,
            minHeight: 0,
            overflow: "auto",
            padding: "16px 20px",
            color: "var(--text-secondary)",
            fontSize: "13px",
            lineHeight: 1.7,
          }}
        >
          {body}
        </div>

        <div
          style={{
            flexShrink: 0,
            padding: "12px 20px",
            borderTop: "1px solid var(--border)",
            display: "flex",
            justifyContent: "flex-end",
            gap: "8px",
          }}
        >
          <button
            onClick={onCancel}
            disabled={busy}
            style={{
              padding: "6px 14px",
              fontSize: "13px",
              backgroundColor: "transparent",
              color: "var(--text-secondary)",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              cursor: "pointer",
            }}
          >
            取消
          </button>
          <button
            onClick={onConfirm}
            disabled={busy}
            style={{
              padding: "6px 14px",
              fontSize: "13px",
              backgroundColor: "var(--danger)",
              color: "#fff",
              border: "none",
              borderRadius: "4px",
              cursor: busy ? "default" : "pointer",
              opacity: busy ? 0.6 : 1,
            }}
          >
            {busy ? "删除中…" : confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}