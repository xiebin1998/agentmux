import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import PlatformCliPicker, { type CliSelection } from "./PlatformCliPicker";
import type { Project, ScopeEntry, SourceCandidates } from "../types";

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
  const [groupIds, setGroupIds] = useState<ScopeEntry[]>([]);
  const [memberIds, setMemberIds] = useState<ScopeEntry[]>([]);
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

  /** 去钉钉按关键词搜群 / 搜人（显式动作，点按钮才调用）。 */
  const searchScope = (kind: "group" | "member", query: string) =>
    invoke<ScopeEntry[]>("search_scope_candidates", { kind, query });

  /**
   * 老数据只存了 id，没有名字和工号。用本机库（会话名 / 历史发送人）补一下，
   * 否则打开编辑时只能看到一串 id —— 正是这次要消掉的东西。
   */
  const fillMissingNames = async (entries: ScopeEntry[]) => {
    const missing = entries.filter((entry) => !entry.name.trim());
    if (missing.length === 0) {
      return entries;
    }
    try {
      const found = await invoke<ScopeEntry[]>("resolve_scope_names", {
        ids: missing.map((entry) => entry.id),
      });
      const names = new Map(found.map((entry) => [entry.id, entry.name]));
      return entries.map((entry) =>
        entry.name.trim() ? entry : { ...entry, name: names.get(entry.id) ?? "" },
      );
    } catch (e) {
      console.error("Failed to resolve scope names:", e);
      return entries;
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
    if (project.dingtalk_cli_path) {
      setImSelection({ platform: project.im_platform || "dingtalk", path: project.dingtalk_cli_path });
    }
    if (project.agent_cli_path) {
      setAgentSelection({ platform: project.agent_platform || "qoder", path: project.agent_cli_path });
    }
    // 范围条目异步补名，补完再落进表单状态。
    const groups = project.group_ids ?? [];
    const members = project.member_ids ?? [];
    setGroupIds(groups);
    setMemberIds(members);
    Promise.all([fillMissingNames(groups), fillMissingNames(members)]).then(
      ([nextGroups, nextMembers]) => {
        setGroupIds(nextGroups);
        setMemberIds(nextMembers);
      },
    );
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
        overflow: "hidden",
      }}
    >
      {/* 三段式：只有中间的表单区滚。整块弹窗滚的话，表单里的下拉一打开
          就会把标题、× 和底部按钮顶出可视区。 */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          backgroundColor: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          borderRadius: "8px",
          width: "560px",
          maxHeight: "85vh",
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

        <div
          style={{
            flex: 1,
            minHeight: 0,
            overflow: "auto",
            overscrollBehavior: "contain",
            padding: "20px",
          }}
        >
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
              （可在「监听日志」里看到丢弃记录）。下面先给本机已有记录，也可按名字去钉钉搜。
            </div>

            <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
              <ScopePicker
                title="指定群"
                kind="group"
                options={candidates.groups}
                selected={groupIds}
                loading={loadingCandidates}
                placeholder="粘贴会话 id"
                searchPlaceholder="搜群名，如：客服"
                onSearch={(query) => searchScope("group", query)}
                onChange={setGroupIds}
              />
              <ScopePicker
                title="指定人"
                kind="member"
                options={candidates.people}
                selected={memberIds}
                loading={loadingCandidates}
                placeholder="粘贴 open id"
                searchPlaceholder="搜姓名或工号"
                onSearch={(query) => searchScope("member", query)}
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
                {loadingCandidates ? "刷新中…" : "刷新本机候选（拉一次会话名）"}
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

            <div style={{ fontSize: "11px", color: "var(--text-muted)", marginBottom: "10px" }}>
              保存后立刻对这个项目正在运行的监听生效，不必重启监听。
            </div>

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
        </div>

        {/* 底部操作固定在弹窗外壳上：不必把表单滚到底才能点 */}
        <div
          style={{
            flexShrink: 0,
            display: "flex",
            gap: "8px",
            justifyContent: "flex-end",
            padding: "12px 20px",
            borderTop: "1px solid var(--border)",
          }}
        >
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
  );
}

/**
 * 「指定群 / 指定人」的选择器。
 *
 * 默认列本机已有的记录（会话列表 + 历史发送人），也可以按名字去钉钉搜：
 * 搜群名走 `chat +chat-search`，搜人走 `contact +search-user`（姓名或工号都行）。
 * 搜索是**显式动作** —— 只有点「搜索」才起 dws 子进程。
 *
 * 列表只显示名字（群还带人数、人还带工号和职位），**不显示 id**；id 放在悬停提示里备查。
 * 手输框保留，用于粘贴 id 兜底（首次使用、或本机与钉钉都搜不到时）。
 */
function ScopePicker({
  title,
  kind,
  options,
  selected,
  loading,
  placeholder,
  searchPlaceholder,
  onSearch,
  onChange,
}: {
  title: string;
  kind: "group" | "member";
  options: ScopeEntry[];
  selected: ScopeEntry[];
  loading: boolean;
  placeholder: string;
  searchPlaceholder: string;
  onSearch: (query: string) => Promise<ScopeEntry[]>;
  onChange: (next: ScopeEntry[]) => void;
}) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<ScopeEntry[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const isSelected = (id: string) => selected.some((entry) => entry.id === id);
  const toggle = (entry: ScopeEntry) =>
    onChange(
      isSelected(entry.id)
        ? selected.filter((item) => item.id !== entry.id)
        : [...selected, entry],
    );

  const runSearch = async () => {
    const keyword = query.trim();
    if (!keyword || searching) return;
    setSearching(true);
    setNotice(null);
    try {
      const found = await onSearch(keyword);
      setResults(found);
      setNotice(
        found.length === 0
          ? `钉钉里没搜到「${keyword}」相关的${kind === "group" ? "群" : "人"}`
          : null,
      );
    } catch (e) {
      setNotice(String(e));
    } finally {
      setSearching(false);
    }
  };

  const add = () => {
    const id = draft.trim();
    if (!id) return;
    if (!isSelected(id)) {
      onChange([...selected, { id, name: "", code: "", extra: "" }]);
    }
    setDraft("");
  };

  const shown = results ?? options;
  const shownIds = new Set(shown.map((entry) => entry.id));
  // 已选但不在当前列表里的（旧数据、或不是这次搜索的结果）也要显示，否则删不掉。
  const extras = selected.filter((entry) => !shownIds.has(entry.id));

  const rowStyle: CSSProperties = {
    display: "flex",
    gap: "6px",
    alignItems: "baseline",
    fontSize: "12px",
    padding: "2px 0",
    cursor: "pointer",
  };
  const metaStyle: CSSProperties = { color: "var(--text-muted)", fontSize: "10px" };

  const renderRow = (entry: ScopeEntry, manual = false) => (
    <label
      key={entry.id}
      // id 只放在悬停提示里，列表上不显示
      title={entry.id}
      style={rowStyle}
    >
      <input
        type="checkbox"
        checked={isSelected(entry.id)}
        onChange={() => toggle(entry)}
        style={{ accentColor: "var(--accent)" }}
      />
      <span style={{ color: "var(--text-primary)" }}>
        {entry.name || "(名称未知)"}
        {manual && <span style={metaStyle}> · 已选</span>}
      </span>
      {entry.code && <span style={metaStyle}>工号 {entry.code}</span>}
      {entry.extra && <span style={metaStyle}>{entry.extra}</span>}
    </label>
  );

  return (
    <div style={{ flex: 1, minWidth: 0 }}>
      <label style={fieldLabel}>{title}</label>

      <div style={{ display: "flex", gap: "6px", marginBottom: "6px" }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              runSearch();
            }
          }}
          placeholder={searchPlaceholder}
          style={{ flex: 1, minWidth: 0, padding: "4px 8px", fontSize: "12px" }}
        />
        <button
          type="button"
          onClick={runSearch}
          disabled={searching || !query.trim()}
          style={{
            padding: "4px 10px",
            fontSize: "12px",
            backgroundColor: searching ? "var(--bg-active)" : "var(--accent)",
            color: "var(--accent-contrast)",
            border: "none",
            borderRadius: "4px",
            cursor: searching || !query.trim() ? "not-allowed" : "pointer",
          }}
        >
          {searching ? "搜索中…" : "搜索"}
        </button>
      </div>

      <div
        style={{
          maxHeight: "130px",
          overflow: "auto",
          border: "1px solid var(--border)",
          borderRadius: "4px",
          padding: "6px 8px",
          backgroundColor: "var(--bg-app)",
        }}
      >
        {loading && shown.length === 0 && (
          <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>加载中…</div>
        )}
        {shown.map((entry) => renderRow(entry))}
        {extras.map((entry) => renderRow(entry, true))}
        {!loading && shown.length === 0 && extras.length === 0 && (
          <div style={{ fontSize: "11px", color: "var(--text-muted)" }}>
            本机还没有记录，可以按名字搜钉钉，或在下面直接粘贴 id
          </div>
        )}
      </div>

      {notice && (
        <div style={{ fontSize: "11px", color: "var(--warn)", marginTop: "4px" }}>{notice}</div>
      )}

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

      {results && (
        <button
          type="button"
          onClick={() => {
            setResults(null);
            setNotice(null);
          }}
          style={{
            marginTop: "4px",
            padding: "0",
            fontSize: "11px",
            backgroundColor: "transparent",
            color: "var(--text-muted)",
            border: "none",
            textDecoration: "underline",
            cursor: "pointer",
          }}
        >
          返回本机候选
        </button>
      )}
    </div>
  );
}
