import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import PlatformCliPicker, { type CliSelection } from "./PlatformCliPicker";
import type { NamedId, Project, SourceCandidates } from "../types";

interface ProjectDialogProps {
  project: Project | null;
  onClose: () => void;
  onSaved: () => void;
}

const TIMEOUT_PRESETS = [60_000, 120_000, 300_000];

const fieldLabel: CSSProperties = {
  display: "block",
  color: "var(--text-secondary)",
  fontSize: "13px",
  marginBottom: "6px",
};

const textInput: CSSProperties = {
  width: "100%",
  padding: "8px 12px",
  fontSize: "13px",
};

export default function ProjectDialog({ project, onClose, onSaved }: ProjectDialogProps) {
  const [name, setName] = useState("");
  const [workDir, setWorkDir] = useState("");
  const [imSelection, setImSelection] = useState<CliSelection | null>(null);
  const [agentSelection, setAgentSelection] = useState<CliSelection | null>(null);
  const [replyEnabled, setReplyEnabled] = useState(false);
  const [replyTimeout, setReplyTimeout] = useState(120_000);
  const [replyMaxChars, setReplyMaxChars] = useState(500);
  const [contextEnabled, setContextEnabled] = useState(true);
  const [contextLimit, setContextLimit] = useState(50);
  const [contextMaxChars, setContextMaxChars] = useState(8000);
  const [groupIds, setGroupIds] = useState<string[]>([]);
  const [memberIds, setMemberIds] = useState<string[]>([]);
  const [candidates, setCandidates] = useState<SourceCandidates>({ groups: [], people: [] });
  const [loadingCandidates, setLoadingCandidates] = useState(false);
  const [saving, setSaving] = useState(false);

  const loadCandidates = async (refreshMeta: boolean) => {
    setLoadingCandidates(true);
    try {
      if (refreshMeta) {
        // 先拉一次会话名/类型，候选列表才有群名可显示（会真起一次 dws，是显式动作）。
        await invoke("refresh_conversation_meta").catch(() => {});
      }
      setCandidates(await invoke<SourceCandidates>("list_source_candidates"));
    } catch (e) {
      console.error("Failed to load source candidates:", e);
    } finally {
      setLoadingCandidates(false);
    }
  };

  useEffect(() => {
    loadCandidates(false);
  }, []);

  useEffect(() => {
    if (!project) {
      return;
    }
    setName(project.name);
    setWorkDir(project.work_dir);
    setReplyEnabled(project.reply_enabled);
    setReplyTimeout(project.reply_timeout_ms);
    setReplyMaxChars(project.reply_max_chars);
    setContextEnabled(project.context_enabled);
    setContextLimit(project.context_message_limit);
    setContextMaxChars(project.context_max_chars);
    setGroupIds(project.group_ids ?? []);
    setMemberIds(project.member_ids ?? []);
    if (project.dingtalk_cli_path) {
      setImSelection({ platform: project.im_platform || "dingtalk", path: project.dingtalk_cli_path });
    }
    if (project.agent_cli_path) {
      setAgentSelection({ platform: project.agent_platform || "qoder", path: project.agent_cli_path });
    }
  }, [project]);

  const handleSubmit = async () => {
    if (!name || !workDir) {
      alert("请填写项目名称与工作目录");
      return;
    }
    if (!imSelection || !agentSelection) {
      alert("请先选择 IM 平台与 Agent 平台的 CLI（若列表为空，请点「重新检测」）");
      return;
    }

    setSaving(true);
    try {
      const payload = {
        name,
        workDir,
        agentCliPath: agentSelection.path,
        dingtalkCliPath: imSelection.path,
        agentPlatform: agentSelection.platform,
        imPlatform: imSelection.platform,
        replyEnabled,
        replyTimeoutMs: replyTimeout,
        replyMaxChars,
        contextEnabled,
        contextMessageLimit: contextLimit,
        contextMaxChars,
        groupIds,
        memberIds,
      };

      if (project) {
        await invoke("update_project", {
          project: {
            ...project,
            ...payload,
            agent_cli_path: payload.agentCliPath,
            dingtalk_cli_path: payload.dingtalkCliPath,
            agent_platform: payload.agentPlatform,
            im_platform: payload.imPlatform,
            work_dir: payload.workDir,
            reply_enabled: payload.replyEnabled,
            reply_timeout_ms: payload.replyTimeoutMs,
            reply_max_chars: payload.replyMaxChars,
            context_enabled: payload.contextEnabled,
            context_message_limit: payload.contextMessageLimit,
            context_max_chars: payload.contextMaxChars,
            group_ids: payload.groupIds,
            member_ids: payload.memberIds,
          },
        });
      } else {
        await invoke("create_project", payload);
      }
      onSaved();
    } catch (e) {
      console.error("Failed to save project:", e);
      alert("保存失败：" + String(e));
    } finally {
      setSaving(false);
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
          width: "560px",
          maxHeight: "85vh",
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
          }}
        >
          <h2 style={{ margin: 0, color: "var(--text-primary)", fontSize: "16px" }}>
            {project ? "编辑项目" : "创建项目"}
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
          <div style={{ marginBottom: "16px" }}>
            <label style={fieldLabel}>项目名称 *</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="例如：我的工作项目"
              style={textInput}
            />
          </div>

          <div style={{ marginBottom: "16px" }}>
            <label style={fieldLabel}>工作目录 *</label>
            <div style={{ display: "flex", gap: "8px" }}>
              <input
                type="text"
                value={workDir}
                onChange={(e) => setWorkDir(e.target.value)}
                placeholder="例如：D:\work\my-project"
                style={textInput}
              />
              <button
                type="button"
                onClick={async () => {
                  try {
                    const picked = await open({
                      directory: true,
                      multiple: false,
                      defaultPath: workDir || undefined,
                      title: "选择 Agent 的工作目录",
                    });
                    if (typeof picked === "string" && picked) {
                      setWorkDir(picked);
                    }
                  } catch (e) {
                    alert("打开目录选择器失败：" + String(e));
                  }
                }}
                style={{
                  padding: "8px 14px",
                  whiteSpace: "nowrap",
                  backgroundColor: "transparent",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  borderRadius: "4px",
                  cursor: "pointer",
                  fontSize: "13px",
                }}
              >
                选择…
              </button>
            </div>
            <div style={{ color: "var(--text-muted)", fontSize: "11px", marginTop: "4px" }}>
              Agent 只在这个目录内读写；驱动 Agent CLI 时就以它为工作目录
            </div>
          </div>

          <PlatformCliPicker
            kind="im"
            title="IM 平台 CLI *"
            hint="自动从全局 PATH 与已知安装位置检测，选中即可，无需手输路径"
            value={imSelection}
            onChange={setImSelection}
          />

          <PlatformCliPicker
            kind="agent"
            title="Agent 平台 CLI *"
            hint="已接入可扩展平台注册表，新平台检测到后会自动出现在这里"
            value={agentSelection}
            onChange={setAgentSelection}
          />

          <div
            style={{
              marginTop: "4px",
              marginBottom: "16px",
              padding: "12px",
              border: "1px solid var(--border)",
              borderRadius: "6px",
            }}
          >
            <div
              style={{
                fontSize: "11px",
                fontWeight: 600,
                color: "var(--text-muted)",
                marginBottom: "6px",
                textTransform: "uppercase",
              }}
            >
              监听范围
            </div>
            <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "10px" }}>
              都不选 = 监听<strong>所有群、所有人</strong>。选了之后只处理命中的消息：命中「指定的群」
              <strong>或</strong>「指定的人」其一即可，其余消息在落盘前就被丢弃
              （可在「监听日志」里看到丢弃记录）。候选来自本机已有的会话与历史发送人。
            </div>

            <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
              <IdPicker
                title="指定群"
                options={candidates.groups}
                selected={groupIds}
                loading={loadingCandidates}
                placeholder="也可手输群会话 id"
                onChange={setGroupIds}
              />
              <IdPicker
                title="指定人"
                options={candidates.people}
                selected={memberIds}
                loading={loadingCandidates}
                placeholder="也可手输 open id"
                onChange={setMemberIds}
              />
            </div>

            <div style={{ display: "flex", alignItems: "center", marginTop: "8px" }}>
              <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                已选：{groupIds.length} 个群 / {memberIds.length} 个人
              </span>
              <button
                type="button"
                onClick={() => loadCandidates(true)}
                disabled={loadingCandidates}
                style={{
                  marginLeft: "auto",
                  padding: "2px 10px",
                  fontSize: "11px",
                  backgroundColor: "transparent",
                  color: "var(--text-secondary)",
                  border: "1px solid var(--border)",
                  borderRadius: "4px",
                  cursor: loadingCandidates ? "not-allowed" : "pointer",
                }}
              >
                {loadingCandidates ? "刷新中…" : "刷新候选（拉一次会话名）"}
              </button>
            </div>
          </div>

          <div
            style={{
              marginTop: "4px",
              marginBottom: "16px",
              padding: "12px",
              border: "1px solid var(--border)",
              borderRadius: "6px",
            }}
          >
            <div
              style={{
                fontSize: "11px",
                fontWeight: 600,
                color: "var(--text-muted)",
                marginBottom: "10px",
                textTransform: "uppercase",
              }}
            >
              回复与上下文
            </div>

            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "8px",
                fontSize: "13px",
                marginBottom: "10px",
              }}
            >
              <input
                type="checkbox"
                checked={replyEnabled}
                onChange={(e) => setReplyEnabled(e.target.checked)}
                style={{ accentColor: "var(--accent)" }}
              />
              启用自动回复
            </label>

            {!replyEnabled && (
              <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "10px" }}>
                未勾选时只记录不回复：消息照常收下并落盘，但不会驱动 Agent 生成回复。
              </div>
            )}

            <div style={{ display: "flex", gap: "16px", marginBottom: "10px" }}>
              <div style={{ flex: 1 }}>
                <label style={fieldLabel}>生成超时</label>
                <select
                  value={replyTimeout}
                  onChange={(e) => setReplyTimeout(Number(e.target.value))}
                  style={{ ...textInput, padding: "6px 8px" }}
                >
                  {TIMEOUT_PRESETS.map((preset) => (
                    <option key={preset} value={preset}>
                      {preset / 1000}s
                    </option>
                  ))}
                </select>
              </div>
              <div style={{ flex: 1 }}>
                <label style={fieldLabel}>回复字数上限</label>
                <input
                  type="number"
                  min={1}
                  value={replyMaxChars}
                  onChange={(e) => setReplyMaxChars(Number(e.target.value))}
                  style={textInput}
                />
              </div>
            </div>

            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "8px",
                fontSize: "13px",
                marginBottom: "10px",
              }}
            >
              <input
                type="checkbox"
                checked={contextEnabled}
                onChange={(e) => setContextEnabled(e.target.checked)}
                style={{ accentColor: "var(--accent)" }}
              />
              注入群上下文（关闭后回答可能缺少上下文）
            </label>

            <div style={{ display: "flex", gap: "16px" }}>
              <div style={{ flex: 1 }}>
                <label style={fieldLabel}>上下文消息条数</label>
                <input
                  type="number"
                  min={1}
                  value={contextLimit}
                  onChange={(e) => setContextLimit(Number(e.target.value))}
                  disabled={!contextEnabled}
                  style={textInput}
                />
              </div>
              <div style={{ flex: 1 }}>
                <label style={fieldLabel}>上下文字符预算</label>
                <input
                  type="number"
                  min={1}
                  value={contextMaxChars}
                  onChange={(e) => setContextMaxChars(Number(e.target.value))}
                  disabled={!contextEnabled}
                  style={textInput}
                />
              </div>
            </div>
          </div>

          <div style={{ display: "flex", gap: "8px", justifyContent: "flex-end" }}>
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
              取消
            </button>
            <button
              onClick={handleSubmit}
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
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 「指定群 / 指定人」的选择器：候选勾选 + 手输 id 兜底。
 * 手输是必要的：首次使用还没有任何会话记录时，候选是空的。
 */
function IdPicker({
  title,
  options,
  selected,
  loading,
  placeholder,
  onChange,
}: {
  title: string;
  options: NamedId[];
  selected: string[];
  loading: boolean;
  placeholder: string;
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState("");
  const has = (id: string) => selected.includes(id);

  const toggle = (id: string) =>
    onChange(has(id) ? selected.filter((item) => item !== id) : [...selected, id]);

  const add = () => {
    const id = draft.trim();
    if (!id) return;
    if (!has(id)) onChange([...selected, id]);
    setDraft("");
  };

  // 已选但不在候选里的（手输的、或候选刷新后消失的）也要显示，否则删不掉。
  const extras = selected.filter((id) => !options.some((option) => option.id === id));

  const idStyle: CSSProperties = {
    color: "var(--text-muted)",
    fontFamily: "ui-monospace, Consolas, monospace",
    fontSize: "10px",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  };

  return (
    <div style={{ flex: 1, minWidth: 0 }}>
      <label style={fieldLabel}>{title}</label>
      <div
        style={{
          maxHeight: "120px",
          overflow: "auto",
          border: "1px solid var(--border)",
          borderRadius: "4px",
          padding: "6px 8px",
          backgroundColor: "var(--bg-app)",
        }}
      >
        {loading && options.length === 0 && (
          <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>加载中…</div>
        )}
        {!loading && options.length === 0 && extras.length === 0 && (
          <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
            暂无可选记录，可在下面手输 id
          </div>
        )}
        {options.map((option) => (
          <label
            key={option.id}
            title={option.id}
            style={{
              display: "flex",
              gap: "6px",
              alignItems: "baseline",
              fontSize: "12px",
              padding: "2px 0",
              cursor: "pointer",
            }}
          >
            <input
              type="checkbox"
              checked={has(option.id)}
              onChange={() => toggle(option.id)}
              style={{ accentColor: "var(--accent)" }}
            />
            <span style={{ color: "var(--text-primary)" }}>{option.name || "(未命名)"}</span>
            <span style={idStyle}>{option.id}</span>
          </label>
        ))}
        {extras.map((id) => (
          <label
            key={id}
            title={id}
            style={{
              display: "flex",
              gap: "6px",
              alignItems: "baseline",
              fontSize: "12px",
              padding: "2px 0",
              cursor: "pointer",
            }}
          >
            <input
              type="checkbox"
              checked
              onChange={() => toggle(id)}
              style={{ accentColor: "var(--accent)" }}
            />
            <span style={{ color: "var(--text-secondary)" }}>(手输)</span>
            <span style={idStyle}>{id}</span>
          </label>
        ))}
      </div>
      <div style={{ display: "flex", gap: "6px", marginTop: "6px" }}>
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          placeholder={placeholder}
          style={{ flex: 1, minWidth: 0, padding: "4px 8px", fontSize: "12px" }}
        />
        <button
          type="button"
          onClick={add}
          style={{
            padding: "4px 10px",
            fontSize: "12px",
            backgroundColor: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            cursor: "pointer",
          }}
        >
          添加
        </button>
      </div>
    </div>
  );
}
