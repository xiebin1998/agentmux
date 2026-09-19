import { useState, type CSSProperties } from "react";

interface Project {
  id: string;
  name: string;
  work_dir: string;
  agent_cli_path: string;
  dingtalk_cli_path: string;
  im_platform: string;
  agent_platform: string;
  reply_enabled: boolean;
  reply_timeout_ms: number;
  reply_max_chars: number;
  context_enabled: boolean;
  context_message_limit: number;
  context_max_chars: number;
  created_at: string;
  updated_at: string;
}

interface ProjectListProps {
  projects: Project[];
  selectedProject: Project | null;
  onSelectProject: (project: Project) => void;
  onEditProject: (project: Project) => void;
  onDeleteProject: (id: string) => void;
}

const iconButton: CSSProperties = {
  padding: "2px 6px",
  backgroundColor: "transparent",
  color: "var(--text-secondary)",
  border: "none",
  borderRadius: "3px",
  cursor: "pointer",
  fontSize: "11px",
};

export default function ProjectList({
  projects,
  selectedProject,
  onSelectProject,
  onEditProject,
  onDeleteProject,
}: ProjectListProps) {
  const [expandedProject, setExpandedProject] = useState<string | null>(null);

  const handleProjectClick = (project: Project) => {
    onSelectProject(project);
    setExpandedProject(expandedProject === project.id ? null : project.id);
  };

  return (
    <div style={{ borderBottom: "1px solid var(--border)" }}>
      <div
        style={{
          padding: "8px 12px",
          fontSize: "11px",
          fontWeight: 600,
          color: "var(--text-secondary)",
          textTransform: "uppercase",
          letterSpacing: "0.5px",
        }}
      >
        项目
      </div>
      {projects.length === 0 ? (
        <div style={{ padding: "12px", color: "var(--text-muted)", fontSize: "13px" }}>
          暂无项目，点击上方「创建项目」按钮
        </div>
      ) : (
        <div>
          {projects.map((project) => {
            const selected = selectedProject?.id === project.id;
            return (
              <div
                key={project.id}
                style={{ backgroundColor: selected ? "var(--bg-active)" : "transparent" }}
              >
                <div
                  onClick={() => handleProjectClick(project)}
                  style={{
                    padding: "8px 12px",
                    display: "flex",
                    alignItems: "center",
                    gap: "8px",
                    cursor: "pointer",
                  }}
                >
                  <span style={{ fontSize: "14px", color: "var(--text-secondary)" }}>
                    {expandedProject === project.id ? "▼" : "▶"}
                  </span>
                  <span
                    style={{ color: "var(--text-primary)", fontSize: "13px", flex: 1 }}
                    title={project.name}
                  >
                    {project.name}
                  </span>
                  <div style={{ display: "flex", gap: "4px" }}>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onEditProject(project);
                      }}
                      style={iconButton}
                    >
                      编辑
                    </button>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onDeleteProject(project.id);
                      }}
                      style={{ ...iconButton, color: "var(--danger)" }}
                    >
                      删除
                    </button>
                  </div>
                </div>
                <div
                  style={{
                    padding: "0 12px 8px 32px",
                    fontSize: "11px",
                    color: "var(--text-muted)",
                    lineHeight: 1.6,
                  }}
                >
                  <div>工作目录: {project.work_dir}</div>
                  <div>
                    Agent: {project.agent_platform ?? "—"} · IM: {project.im_platform ?? "—"}
                  </div>
                  <div>回复: {project.reply_enabled ? "开启" : "关闭"}</div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
