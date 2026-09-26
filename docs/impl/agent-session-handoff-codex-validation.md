# Codex 源会话稳定检测验证（2026-09-25）

基线：main `726ddcf6`，workspace 922 passed / 0 failed。
范围：P0-A 对端、P0-B 空 libproc 回退、P1-E recent 建议与确认；不修改投递 argv。

## 实现与测试

| 项 | 文件（相对于 handoff 目录） | 行为与新增测试 |
| --- | --- | --- |
| P0-A | con-agent `codex/peers.rs`、`codex/process.rs`、`codex/evidence.rs`、`codex.rs`、`binding.rs` | 数字化 lsof TCP 快照匹配完整反向 loopback 地址/端口对；只接纳 ESTABLISHED、argv 为 codex app-server 的 peer。保留 leader→pgid 回退，扫描 peer 的持有文件；跨来源 ID 冲突报错，每个来源内 rollout 优先。前后复查连接与进程身份。6 个新增测试覆盖多 TUI、共享端口、IPv6、无连接、端口复用、代理排除、多 owner、peer 冲突及 mock TUI→peer rollout 整链。 |
| P0-B | con-agent `codex/process.rs`、`codex/lsof.rs`；con-app `destination/binding_check.rs` | libproc 空结果和 Err 均走 lsof，只有 Err 警告；lsof 最长 5 秒，整体探测最长 15 秒，有输出上限。仅无 stdout/stderr 的退出 1 视作空选择，错误/部分结果阻断 recent。3 个新增测试覆盖空/失败/非空 libproc 和 lsof 状态；扩展既有 hint 测试。 |
| P1-E | con-agent `codex/recent.rs`、`binding.rs`；con-app `destination.rs`、`destination/{prepare,lifecycle,binding_check}.rs`、`route/mod.rs` | 仅探测成功且无证据时 discover 同 cwd 会话，最新正数 updatedAt 唯一才建议，始终 requires_confirmation=true，evidence 含 confirm it is current。复用既有勾选 UI；prepare 前后及最终路由禁止从 lock 静默降级为未确认 recent。5 个核心测试和 1 个确认降级测试。 |
| P2 | 本记录、设计文档 | 隔离 HOME/CODEX_HOME、子进程入口 env -i，读取本机 codex-cli 0.157.0 的 help 并 generate-json-schema --experimental。无当前 TUI thread 方法；thread/loaded/list 仅列内存已加载线程，不能替代当前线程。待上游确认，未接入猜测 API。 |

既有 3 个 evidence 测试仅移动到 `codex/evidence.rs`，rollout 优先、双锁拒绝仍通过。
共新增 15 个测试，未删除旧测试。新模块及本次修改的代码文件均少于 500 行。
Cursor/Kimi 的 binding 分支不变；其已有确认操作仍满足复核门槛。

## 契约清单

原行号按 round6 报告；实现拆文件后的定位以符号为准。

| # | 原锚点 | 完成 |
| --- | --- | --- |
| 1 | contract.md:48 | ✅ 增加 app-server TCP 对端与身份校验，保留 pgid，定义跨来源冲突拒绝 |
| 2 | contract.md:5–8 | ✅ Codex recent 建议+确认，与 Cursor 对称；错误不走建议 |
| 3 | contract.md:338–339 | ✅ 禁止无确认 recent；区分无 lock、多证据、探测失败、未连接 |
| 4 | plan.md:17 | ✅ 改验收为 TUI 的对端 rollout/lock，纯 TUI 无锁不算失败 |
| 5 | agent-session-handoff.md:106–116 | ✅ 记录 0.157 daemon 分体与 discover/bind 差异 |
| 6 | codex.rs:41–57 | ✅ 委托 process.rs；空 libproc 不跳过 lsof |
| 7 | binding.rs:62–67 | ✅ Codex 委托 recent.rs；错误直返、证据优先、建议需确认 |
| 8 | binding_check.rs:49–61 | ✅ 未连接、无锁、多锁/rollout/冲突、探测失败独立提示 |

## 五项验证结果

| 验证 | 结果 |
| --- | --- |
| `mise exec -- cargo build --workspace` | ✅ exit 0，最终代码再次构建通过 |
| `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` | ✅ exit 0 |
| `mise exec -- just lint` | ✅ exit 0 |
| `mise exec -- cargo test --workspace` | ✅ exit 0；937 passed / 0 failed / 0 ignored，比基线 +15 |
| 活进程只读验证 | **部分完成**：实际文件扫描与 lsof 一致；没有独立用户 TUI，TUI→peer 与面板仍待桌面验收 |

上述四个构建/检查命令首次在沙箱内均遇到 Clang ModuleCache 写权限限制，
根因是 `~/.cache/clang/ModuleCache/...pcm: Operation not permitted`。
授权沙箱外重跑后全部成功；未修改工具链、依赖或编译配置。
保留既有 Cargo 双 binary manifest 提示及第三方 future-incompatibility 提示。
`cargo fmt --all -- --check`、`git diff --check` 同样通过。

活进程快照（只记录身份和 ID，不读取会话正文）：

| PID | 身份 | 新 `active_codex_thread_id` |
| --- | --- | --- |
| 75726 | codex app-server --listen（managed daemon） | `Ok(None)` |
| 75783 | codex app-server daemon | `Ok(None)` |
| 91730 | 工具宿主 app-server，并非独立 TUI | `Ok(Some("01a0d7d2-7c0e-78d3-85cc-7f26022153d0"))` |

`lsof -nP +D ~/.codex/thread-writer-locks -Fpn` 同期显示 PID 91730 持有
`01a0d7d2-7c0e-78d3-85cc-7f26022153d0.lock`，rollout 同 ID。
原报告 TUI 73290 已退出；当前没有能完成“独立 TUI→managed peer”的活体样本。
临时 Rust example 只调用新公开检测入口，验证后已移除。
启动探针时入口 `env -i` 清空环境，HOME 为 `/private/tmp/con-codex-schema-home`，
CODEX_HOME 显式指向真实存储且仅由文件路径检测读取；无 resume、无会话写入、无 lock 修改、无进程终止。
P2 的 CODEX_HOME 也使用临时目录，schema 输出位于 `/private/tmp/con-codex-schema`。

## 手工验收

1. 在 Con Tab 中运行用户自己的 Codex 0.157，同 cwd 保留至少两条历史会话。
   打开 Handoff；有唯一有效证据时应自动预选当前会话，不要求 recent 确认。
   对照 TUI 的 ESTABLISHED 反向 TCP 对端和该 peer 的 rollout/writer lock ID。
2. 在自然无锁、探测成功且最新 updatedAt 唯一的场景重开面板，应显示
   “Recent Codex session — confirm it is current”，未勾选不能 Send，勾选后可继续。
   不得为了验收删除真实 lock；可用入口清空环境的隔离会话验证。
3. 并列更新时间、多证据冲突或探测失败时不得无确认自动绑定；检查独立 hint。
   多 TUI/同 cwd 时不能串 Tab；面板打开后证据消失、会话切换或确认要求升级，应拒绝发送并要求重开。

## 边界与未完成项

- 未做真实桌面自动预选及勾选交互验收；不以 daemon 文件扫描或 mock 测试替代。
- P2 未找到当前 TUI thread 的只读契约，待上游确认。loaded/list 不用于自动绑定。
- 仍沿用 Con 自身 CODEX_HOME；按源进程环境解析 CODEX_HOME 是 round6 的 P3，不在本次实现范围。
- 探测平台为 macOS；未新增 Windows/Linux 的进程探测实现。
- 不改六条 NIT、不修改 `work/reviews/`；不运行目标 resume、无 push/PR/新分支。
- Codex/Cursor argv 路径零改动；`then_some(prompt)`、`automatic_delivery` 门控、
  `codex_automatic_prompt_is_one_literal_argument` 原样，指定测试通过。
- Nowledge 已做定向线程与 memory 检索，未返回本任务相关旧决策；采用用户指定 round6 报告。
  当前未能核验图谱浏览器身份及 Space 权限，未展开图谱。
