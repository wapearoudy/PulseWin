# Token 来源移植状态

对照 Pulse v1.9.0：54 个 Token 来源**全部登记**（`src-tauri/src/token_spend.rs` 的 `AGENTS` 目录，54 项）。
没有 reader 的来源不会进入这个目录，因此「已登记」等于「有读取器」，不再是「先占位」。

## 0.1.17 之后新移植的 13 个来源

| 来源 | id | 读取位置 | 说明 |
| --- | --- | --- | --- |
| Grok Build | `grok` | `~/.grok` 会话记录 | 与 Codex 相似的单会话账本 |
| Devin | `devin-cli` | Devin CLI 数据库 | 原版把 CLI 与桌面版合并成同一个来源名，数据库本身无法区分是哪个客户端写入 |
| Devin Desktop | `devin-desktop` | `%APPDATA%` 捕获树 | CLI 数据库只用来核对会话身份，不作为数据来源 |
| CodeBuddy | `codebuddy` | `~/.codebuddy` | 会话记录优先，扩展日志与聚合库只作兜底 |
| WorkBuddy | `workbuddy` | `~/.workbuddy`、`~/.workbuddy-ai` | 同上 |
| Antigravity CLI | `antigravity-cli` | 单一捕获目录 | 自带最小 protobuf 解码，只统计新输入 / 缓存读 / 输出 |
| Antigravity IDE | `antigravity-ide` | 两个独立捕获目录 | 与 CLI 不共用目录，因此不会互相计数 |
| Trae | `trae` | 分页摘要 | 摘要存在时跨页去重；缓存命中会削弱这部分核对 |
| Warp | `warp` | 快照 | **不产生 token 用量**：快照只报请求数与花费 |
| Copilot | `copilot` | 产品自身的三类存储 | 三条泳道合并成一次读数，不按文件相加，避免同一次调用计三次 |
| OmO Native | `senpi` | `~/.senpi/agent/sessions` 加每个 cwd 下的 `.omo/senpi-task/children` | 子会话在会话树之外，所以不能复用 Pi 的目录 |
| Prime Agent | `prime-agent` | `~/.prime/agent/sessions`、`session-artifacts` 及设置重定向 | 父消息的 `aggregateUsage` 已含子会话用量，按 `child_usage_attributed` 逐字段扣减并夹到 0；子会话缺失时父消息保留原值 |
| Freebuff | `freebuff` | 与 Codebuff 相同的 `manicode*` 树 | **不产生 token 用量**：只存字符估算，原版明确不当作已测用量呈现 |

## 不产生数字的来源

Warp 与 Freebuff 在原版里就是「不报告 token 计数」（`SpendAgent.reportsTokenCounts` 为 false）：
Warp 的快照只有请求数和花费，Freebuff 只有按字符估算的量。移植后它们仍登记在目录里、
仍出现在 Token 页面的来源列表中，但读数固定为 0 并附一条说明，不把推测值画成数字。

## 需要用户提供导出文件的来源

Cursor、Antigravity（导出）、Hindsight、MiniMax Code 仍是 tokscale 捕获/导出格式，
需要用户把导出文件放进 `%APPDATA%/PulseWin/UsageImports/<client>` 或设置 `TOKSCALE_CONFIG_DIR`。
这是原版行为，不是移植遗漏。

## 已知边界

- 逐来源的真实用户日志验收尚未完成：现有测试是 fixture 级别的，证明格式解析、去重、
  取消、只读与 WAL / 时间侧文件失效处理正确，不证明每个用户机器上的日志都能读。
- 缓存命中时的降级：Trae 的跨页去重依赖摘要，摘要被缓存（同一次扫描不再重算）时
  重叠页可能重复计入；已在 reader 中标注，没有静默改变语义。
