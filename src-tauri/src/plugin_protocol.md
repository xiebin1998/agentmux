# AgentMux 外部插件协议 v1

外部插件是一个**可执行程序**（任意语言），宿主通过 **stdio 上的 NDJSON** 与它通信。
插件与宿主是两个进程，插件崩溃不会带崩宿主——这是「双轨插件」里外部轨的全部价值。

## 一、目录约定

只支持**一个**插件根目录（v1 边界）：

```
<data_dir>/plugins/<你的插件目录>/manifest.json
```

`<data_dir>` 在「设置 → 数据与诊断」里可见。

## 二、manifest.json

```json
{
  "id": "my-agent",
  "name": "My Agent",
  "version": "0.1.0",
  "protocol": 1,
  "kind": "agent",
  "command": "node",
  "args": ["index.js"],
  "capabilities": ["generate"]
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 全局唯一；握手回包必须原样带回 |
| `name` | 是 | 界面展示名 |
| `version` | 否 | v1 只展示，不做版本校验 |
| `protocol` | 否 | 协议版本，默认 `1`；用于协商 |
| `kind` | 是 | `agent` 或 `im` |
| `command` / `args` | 是 | 宿主直接 spawn，**不经 shell**；`.cmd/.bat/.ps1` 包装脚本不能直接启动 |
| `capabilities` | 否 | 决定风险分级，见下 |

## 三、握手（宿主 5 秒超时）

宿主 spawn 插件后，向它的 **stdin** 写一行：

```json
{"type":"hello","protocol":1}
```

插件必须在 **5 秒内**于 **stdout** 回一行：

```json
{"type":"hello","protocol":1,"id":"my-agent"}
```

- `type` 必须是 `hello`，`id` 必须与 manifest 一致；
- 超时、非 JSON、字段不匹配，都判定为**握手失败**，界面会显示原始回包/错误原因；
- 宿主随后会结束该进程——握手只用于「探测能不能跑」，不代表插件已被装载执行。

## 四、风险分级

按 manifest 声明的能力分级，界面上如实标注：

| 级别 | 触发能力 |
|---|---|
| 高 | `send_message`、`exec`、`shell`、`file_write`、`network` |
| 中 | `generate`、`read_history` |
| 低 | 其它 / 未声明 |

v1 只做**分级提示**，不拦截。装机前请自行确认来源可信。

## 五、v1 未实现（不要假设可用）

- 没有插件调用面：宿主目前不会把事件派发给外部插件，也不会调它的 `generate`；
- 没有权限沙箱、没有签名校验、没有版本协商失败回滚；
- 启用/停用只影响记录状态，热生效范围有限。

这些属于后续版本，不是本版承诺。
