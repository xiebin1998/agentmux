# AgentMux 项目进展总结

**更新时间**: 2026-09-19（第三轮：实测 + 需求功能补齐 + 压缩/插件/导入）

> 第一版文件曾声称「项目已完成」，与事实不符：当时 `orchestrator` 的启停是占位实现、`main.rs` 硬编码 dws 路径。以下为勘正后的真实状态。

---

## 一、实测（用户要求 1）

用 `dws event +listen-im --duration` 跑真实监听：

| 项 | 结果 |
|---|---|
| `@我` 监听 | `ready event_key=user_im_message_receive_at subscribe_id=subId-3a58… bus_pid=22304` |
| `单聊` 监听 | `ready event_key=user_im_message_receive_o2o_all subscribe_id=subId-ae14… bus_pid=22304` |
| 多路并发 | 共享 bus_pid、各自独立 subscribe_id |
| 收尾 | 到期自动退出，`dws event status` 显示 `Subscriptions: none`，无残留订阅 |
| 新发现 | ready 后打印 `idle_timeout=300s` → 长连接空闲 5 分钟会断，**D-33 的退避重连是必需项** |
| **未闭环** | **真实消息到达未验证**：多个窗口期内共收到 0 条事件。需要一条真实的 @我 / 单聊消息才能证明端到端可用 |

---

## 二、需求功能覆盖（A1~A9，67 项）

| 菜单 | 状态 | 实现载体 |
|---|---|---|
| A1 概览 | ✅ | `OverviewView.tsx`：监听总览、统计、一键重检全部提供方（串行）、最近异常（上限 200，与归档同源） |
| A2.1 提供方检测 | ✅ | `ProvidersView.tsx`：未安装/未登录分列徽标、版本、路径、来源、包装脚本提示、原始诊断 JSON、单项重检 |
| A2.2 提供方配置 | ✅ | `PlatformCliPicker.tsx`：**单选，无需手输路径**；Agent 启动参数覆盖（`agent_args`） |
| A3.1 监听开关 | ✅ | 顶栏 @我/单聊 启停 + 状态；多路并发；ready 门控（30s） |
| A3.2 监听日志 | ✅ | `ListenerLogs.tsx`：stderr+运行事件实时日志（后端 5000 行缓冲）、退避过程、**恢复期丢消息明示** |
| A4.1 实时事件流 | ✅ | `EventStream.tsx`：Channel 推流 + 轮询、原文展开 |
| A4.2 事件归档 | ✅ | 会话/发送人/正文检索、**日期范围（31 天上限）**、导出 ndjson、畸形事件记录、跨监听去重 |
| A5.1 回复开关与队列 | ✅ | 总开关 + 全局串行队列 |
| A5.2 防循环保障 | ✅ | 跳过自己（身份可一键回填）、message_id 去重、正文剔除 @ |
| A5.3 回复结果 | ✅ | `ReplyHistory.tsx`：状态、失败原因原文、发送身份核对 + 一键采用 |
| A6.1 会话列表 | ✅ | 会话汇总列表、**Agent 会话状态（已建档/未建档 + sessionId + cwd）**、作废重建 |
| A6.2 会话内容 | ✅ | 消息流、区分对方/机器人、**分页回溯（加载更多）**、上下文范围说明 |
| A7 压缩 | ✅ | 自动（阈值留空则不触发，D-59 未定值不拍脑袋）+ 手动（二次确认）+ 摘要查看/编辑/删除 + 失败不阻塞 + 连续失败 3 次自动暂停 |
| A8 插件 | ✅ | `PluginsPanel.tsx`：内置/外部分区、**真实 5s 握手**、按声明能力风险分级、启停、协议文档（`plugin_protocol.md`） |
| A9.1 通用设置 | ✅ | 自身身份、工作目录、超时预设 60/120/300、字数上限、上下文开关与预算、**运行期真实生效值展示** |
| A9.2 数据与诊断 | ✅ | 配置/数据/归档路径、**导入旧版数据**（事件/台账/会话，不导日志）、单实例保护按 D-79 明确不提供 |

**结论**：67 项功能均有对应实现。剩下的不是"没做"，而是两类**验证强度**问题：
1. 端到端消息闭环未实测（需真实消息）；
2. 部分行为（压缩阈值、断连保活）按台账本身就是待决项，实现上刻意留空而不是编造默认值。

---

## 三、验证证据

| 验证 | 命令 | 结果 |
|---|---|---|
| 后端单测 | `cargo test` | **19 passed / 0 failed** |
| 前端类型+构建 | `npm run build`（`tsc && vite build`） | 通过，48 modules |
| 应用启动 | `target/debug/agentmux.exe` | 正常启动无 panic |
| **主题三态** | 浏览器实测 `data-theme` + `getComputedStyle` | 浅色 `rgb(255,255,255)` / 深色 `rgb(30,30,30)` / 跟随系统回落 light ✅ |
| CLI 自动解析 | `resolve::tests` | 首选候选是真实 `dws.exe`（非包装脚本）、版本可解析、`auth_state=logged_in` |
| 插件握手 | `plugins::tests` | 真实 spawn node 插件，成功/ID 不匹配/缺 command 三条路径均按预期 |
| 旧版导入 | `legacy::tests` | 用**真实** `dingtalk-event-host/data` 导入 + 二次导入全部判重 |

前端构建曾因 `node_modules/typescript` 缺 `bin/tsc` 而必然失败，已 `npm install` 修复（TypeScript 5.9.3）。

---

## 四、运行方式

```bash
cd src-tauri && "$USERPROFILE/.cargo/bin/cargo.exe" test      # 后端测试
cd frontend && npm run build                                  # 前端类型检查 + 构建
cd src-tauri && "$USERPROFILE/.cargo/bin/cargo.exe" tauri dev # 完整开发模式
```

> 注意：`cargo build/test` 前先确认没有残留 `agentmux.exe` 在跑，否则会 `LNK1104` 链接失败。
