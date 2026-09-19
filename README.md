# AgentMux

把"某个 IM 里有人 @我 → 交给本机 AI Agent 处理 → 把回复发回原会话"从一次性脚本升级为**常驻、可插拔、带界面**的桌面客户端。

## 技术栈

- **后端**: Rust + Tauri v2
- **前端**: React + TypeScript + Vite
- **IM 接入**: 钉钉 (通过 `dws` CLI)
- **Agent 接入**: Qoder CLI

## 项目结构

```
agentmux/
├── src-tauri/          # Rust 后端
│   ├── src/
│   │   ├── main.rs           # 应用入口
│   │   ├── providers/        # IM/Agent 提供方检测
│   │   ├── orchestrator/     # 监听管理与事件处理
│   │   ├── storage/          # SQLite 存储 + ndjson 归档
│   │   └── config/           # 配置管理
│   ├── Cargo.toml
│   └── tauri.conf.json
├── frontend/           # React 前端
│   ├── src/
│   │   ├── App.tsx
│   │   └── pages/      # 9 个功能页面
│   ├── package.json
│   └── vite.config.ts
└── docs/               # 需求文档
    └── 需求/
```

## 安装

### 1. 安装 Rust

```bash
# Windows
winget install Rustlang.Rustup

# macOS/Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. 安装前端依赖

```bash
cd frontend
npm install
```

### 3. 编译后端

```bash
cd src-tauri
cargo build
```

## 运行

### 开发模式

终端 1 - 启动前端开发服务器：
```bash
cd frontend
npm run dev
```

终端 2 - 启动 Tauri 应用：
```bash
cd src-tauri
cargo tauri dev
```

### 生产构建

```bash
cd src-tauri
cargo tauri build
```

构建产物在 `src-tauri/target/release/bundle/` 目录下。

## 功能页面

1. **概览** - 运行总览、提供方状态、监听状态、统计
2. **提供方** - IM/Agent 提供方检测与配置
3. **监听** - 启动/停止监听，查看监听状态
4. **事件** - 实时事件流与历史事件查询
5. **回复** - 回复开关、队列、历史
6. **会话** - 会话列表与内容查看
7. **压缩** - 压缩策略与产物管理
8. **插件** - 插件列表与管理
9. **设置** - 通用设置与数据管理

## 需求状态

### 已完成

- ✅ 67/67 功能需求细化完成
- ✅ 73/79 待决项已解决（66 条按建议执行 + 7 条实测解决）
- ✅ 实测验证：
  - `dws version` 命令可用
  - `dws auth status` 命令可用
  - `all-direct` 单聊监听订阅成功
  - 多监听进程可并发（共享 bus_pid，独立 subscribe_id）
  - `--limit` 无硬性上限，单页最多约 50 条

### 代码实现

- ✅ Rust + Tauri v2 项目骨架
- ✅ 核心 trait 定义（ImProvider / AgentProvider / DetectReport）
- ✅ 钉钉适配器（检测 dws 路径、版本、登录态）
- ✅ Qoder 适配器（检测 qodercli 路径）
- ✅ Orchestrator（监听启停、全局去重、事件解析）
- ✅ Storage（SQLite + ndjson 归档）
- ✅ Config（配置加载/保存）
- ✅ 前端 9 个页面全部实现（Overview, Providers, Listeners, Events, Replies, Sessions, Compression, Plugins, Settings）
- ✅ 前端开发服务器可正常启动

### 待完成

- ⏳ 安装 Rust 后编译后端（当前环境未安装 Rust）
- ⏳ 6 条待决项需长期观察（D-33 断连重连、D-59/D-75 压缩阈值、D-76 生效方式）
- ⏳ 用户配合测试单聊 conversation_id 格式（D-23）

## 配置

配置文件位置：
- Windows: `%APPDATA%\agentmux\settings.json`
- macOS/Linux: `~/.config/agentmux/settings.json`

数据目录：
- Windows: `%APPDATA%\agentmux\data\`
- macOS/Linux: `~/.local/share/agentmux/data/`

## 依赖

### 外部 CLI

- **dws** (DingTalk Workspace CLI): 用于钉钉 IM 监听与消息发送
- **qodercli**: 用于 AI Agent 生成回复

### 系统要求

- Rust 1.70+
- Node.js 18+
- Windows 10+ / macOS 10.15+ / Linux (GTK 3)

## 开发笔记

### 关键设计决策

1. **多监听可并发**: 实测验证多个 `dws event +listen-im` 进程可并发共存，共享同一 bus_pid
2. **全局去重**: 使用 `message_id` 集合进行跨监听去重
3. **精准退订**: 停止监听时使用 `subscribe_id` 精准退订，不使用 `--all`
4. **就绪门控**: 监听必须收到 `[event] ready` 信号才算生效
5. **停机阶梯**: 关 stdin 等 10s → kill 等 3s → 精准退订

### 已知限制

- Rust 未安装时无法编译后端
- 需要用户配合测试单聊监听的 conversation_id 格式（D-23）
- 6 条待决项需长期观察或实际使用才能确定（压缩阈值、上下文影响等）

## 许可证

MIT
