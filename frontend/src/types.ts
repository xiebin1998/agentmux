/** 前后端共享的数据形状：以 Rust 侧的 serde 输出为准，别在各组件里各写一份。 */

/** 监听范围里的一个条目：id 用于过滤，其余字段只用于显示（界面不展示 id）。 */
export interface ScopeEntry {
  /** 群的会话 id，或人的 open id */
  id: string;
  /** 群名 / 姓名；老数据可能为空，加载时用本机数据补 */
  name: string;
  /** 工号（人）；群为空 */
  code: string;
  /** 区分信息：群是「42人」，人是职位 */
  extra: string;
}

export interface Project {
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
  /** 监听范围：只处理这些群；空数组 = 不限群 */
  group_ids: ScopeEntry[];
  /** 监听范围：只处理这些人；空数组 = 不限人 */
  member_ids: ScopeEntry[];
  created_at: string;
  updated_at: string;
}

export interface Session {
  id: string;
  project_id: string;
  name: string;
  conversation_id: string;
  created_at: string;
  /** group / direct / unknown，用来打「群聊·单聊」标签 */
  kind: string;
  /** 是否已知名字；未知时界面回退显示会话 id */
  name_known: boolean;
}

/** 候选项：id 是会话 id 或 open id，name 只用于显示。 */
export interface NamedId {
  id: string;
  name: string;
}

export interface SourceCandidates {
  groups: ScopeEntry[];
  people: ScopeEntry[];
}
