import type { ReactNode } from "react";

interface OverlayPanelProps {
  title: string;
  width?: string;
  onClose: () => void;
  children: ReactNode;
}

/**
 * 从顶栏打开的小弹窗（运行总览 / 全部事件）。
 *
 * 三段式：标题与关闭按钮固定，只有中间内容区滚 —— 与设置弹窗一致，
 * 否则内容里的下拉/长列表会把标题顶出可视区。
 */
export default function OverlayPanel({ title, width = "760px", onClose, children }: OverlayPanelProps) {
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
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          backgroundColor: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          borderRadius: "8px",
          width,
          maxWidth: "92vw",
          maxHeight: "88vh",
          overflow: "hidden",
          boxShadow: "var(--shadow)",
        }}
      >
        <div
          style={{
            flexShrink: 0,
            padding: "14px 20px",
            borderBottom: "1px solid var(--border)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <h2 style={{ margin: 0, color: "var(--text-primary)", fontSize: "15px" }}>{title}</h2>
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
        <div style={{ flex: 1, minHeight: 0, overflow: "auto", overscrollBehavior: "contain" }}>
          {children}
        </div>
      </div>
    </div>
  );
}