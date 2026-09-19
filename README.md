# AgentMux

把「某个 IM 里有人 @我 → 交给本机 AI Agent 处理 → 把回复发回原会话」从一次性脚本，
做成**常驻、可插拔、带界面**的桌面客户端。

## 技术栈

| 层 | 选型 |
|---|---|
| 后端 | Rust + Tauri v2 |
| 前端 | React + TypeScript + Vite |
| IM 接入 | 钉钉（通过 `dws` CLI） |
| Agent 接入 | Qoder CLI / Claude Code / Codex（平台注册表可扩展） |
| 存储 | SQLite（查询） + ndjson（按日归档） |

## 核心概念

```
项目（Project）  工作目录 + IM/Agent CLI + 回复设置 + 自己的监听开关
   └── 会话（Session）  对应一个钉钉会话
          └── 消息（Message）  收到的事件与机器人的回复
```

**监听、回复设置、工作目录都是按项目隔离的。** 每个项目各自控制「@我」与「单聊」
两个开关，可同时运行。驱动 Agent CLI 时的**工作目录就是项目创建时指定的 `work_dir`**。

## 快速开始

```bash
# 1. 前端依赖（首次）
cd frontend && npm install

# 2. 开发模式（会自动起 Vite、编译 Rust、打开窗口）
cd ../          # 回到仓库根目录再执行
cargo tauri dev
```

首次使用在界面里：

1. **提供方检测** —— 确认钉钉 CLI 与 Agent CLI 都被检测到（列表里单选即可，不用手输路径）
2. **创建项目** —— 填名称、选工作目录（可手输也可点「选择…」）、选 IM/Agent 平台
3. **启动监听** —— 在左侧**项目行内**点该项目自己的「@我」或「单聊」

更详细的命令、打包与排障见 **[docs/开发与打包指南.md](docs/开发与打包指南.md)**；
模块与数据流见 **[docs/架构/架构说明.html](docs/架构/架构说明.html)**。

## 项目结构

```
agentmux/
├── src-tauri/src/
│   ├── main.rs            应用入口：状态装配、托盘、关闭拦截、命令注册
│   ├── resolve.rs         平台注册表 + CLI 自动解析（按命令名探测、解析可启动路径）
│   ├── reply.rs           回复引擎：生成 → 清洗 → 发送（prompt 走 stdin）
│   ├── orchestrator/      监听生命周期：spawn / ready 门控 / 退避 / 停机阶梯 / 去重
│   ├── storage/           SQLite 事件与会话、摘要、ndjson 归档、迁移
│   ├── config/            全局设置 + 「项目优先、全局兜底」的设置解析
│   ├── project.rs         项目模型与存储
│   ├── plugins.rs         内置适配器清单与启停（外部插件本版不提供）
│   ├── legacy.rs          旧版 dingtalk-event-host 数据导入
│   ├── process.rs         子进程统一隐藏控制台窗口
│   └── providers/         IM/Agent 提供方检测
├── frontend/src/
│   ├── App.tsx            三栏布局、项目级监听控制、关闭询问
│   ├── theme.tsx          主题三态（深色/浅色/跟随系统）
│   └── components/        项目树、消息流、事件流、回复历史、监听日志、
│                          提供方检测、适配器、设置、压缩、CLI 单选、项目对话框
├── .github/workflows/     Release：打 tag 自动构建并发布安装包
└── docs/                  开发与打包指南、架构文档、需求归档
```

## 数据与配置位置

| 内容 | 路径 |
|---|---|
| 配置 | `%APPDATA%\agentmux\settings.json` |
| 数据库 | `%APPDATA%\agentmux\data\agentmux.db` |
| 归档 | `%APPDATA%\agentmux\data\archive\<日期>.ndjson` |

> 自动回复**默认关闭**（只记录不发送）。要在项目的「编辑」里勾选启用。

## 后台常驻

点窗口的 × 会弹窗让你二选一：**最小化到托盘**（继续在后台接收与回复）或**退出**。
托盘图标左键可唤回主窗口，右键菜单提供「显示主窗口 / 退出」。

## 测试与打包

```bash
cd src-tauri && cargo test        # 单元 + 桩驱动的端到端测试
cd frontend  && npm run build     # tsc 类型检查 + vite 构建

cd .          # 回到仓库根目录（打包必须在根目录执行）
cargo tauri build                 # 生成 exe + msi + nsis
cargo tauri build --no-bundle     # 只要免安装 exe
```

首次打安装包需要从 GitHub 下载 WiX 与 NSIS；网络不通时挂代理：

```bash
HTTP_PROXY=http://127.0.0.1:7897 HTTPS_PROXY=http://127.0.0.1:7897 cargo tauri build
```

## 发版

推送 `v*` 形式的 tag（或在 Actions 里手动触发）即可：云端在 `windows-latest` 上构建，
生成 MSI 与 NSIS 安装包并创建 Release 草稿。用的是 Actions 自带的 `GITHUB_TOKEN`，
**不需要任何个人令牌**。

```bash
git tag v0.1.0 && git push origin v0.1.0
```

## 关键设计决策

1. **监听归属项目**：`start_listener(project_id, kind, settings)`，事件带 `project_id`，
   左树按项目聚合会话
2. **多监听可并发**：实测两路 `dws event +listen-im` 共享同一 bus_pid、各自独立 subscribe_id
3. **就绪门控**：必须收到 `[event] ready` 才算监听生效（30s 超时）
4. **精准退订**：按 `subscribe_id` 退订，绝不 `event stop --all`
5. **停机阶梯**：关 stdin 等 10s → 按 pid `taskkill /T /F`
6. **全局去重**：跨监听按 `message_id` 去重
7. **子进程隐藏窗口**：统一 `CREATE_NO_WINDOW`，避免闪黑框
8. **只启动能启动的候选**：`.ps1` / 无扩展名脚本不进候选（`cmd /C` 会把它当文档打开）

## 已知限制

- **外部插件本版不提供**（仅内置适配器）
- **改回复设置需重启该项目的监听**才生效
- 升级前的历史事件 `project_id` 为空，不会出现在项目树下（可用「导入旧版数据」重新归类）
- 以下待决项需长期观察或实际使用才能定：
  D-23 单聊 conversation_id 形态、D-33 断连重连策略、D-59 压缩阈值、
  D-75 关闭上下文对回答质量的影响、D-76 哪些改动需要重启

## 许可证

Apache License 2.0，见 [LICENSE](LICENSE)。
