# Agent Handoff：实施与验收记录

## 当前支持范围（2026-09-25）

支持范围已按用户产品决策收缩为 **Codex、Cursor、Kimi** 三种：
DimAgent（已移除）、OpenCode、Pi、Gemini、GitHub Copilot、Grok 的来源 reader 与目标启动适配已删除
（`AgentKind`、来源模块、探测/argv/候选模型分支与 UI/CLI 选项）。
同日还移除了发送前的 REVIEW & SEND 预览确认页：主表单的 **Send handoff** 一次点击
即 prepare 并直接投递，见文末「移除发送前预览确认页」。
以下历史章节中的“9 种 / 9x9 / 9/9”以及“预览区域 / 预览复核”等描述反映
2026-09-22 至 2026-09-24 期间的状态，仅作历史记录；当前契约见
[架构契约](../design/agent-session-handoff-contract.md) 的“支持范围的收缩”与
“投递确认语义”两节。

## Tab 路由变更计划（2026-09-23，取代旧 UI 验收步骤）

自动识别验收：同一 cwd 至少有两条历史记录时，运行中的 Codex TUI 的 app-server TCP 对端持有哪条 rollout/writer lock，弹窗就直接显示并选中哪条（保留 rollout 优先和进程组回退）。Codex 0.157 的纯 TUI PID 无 lock 不算失败；对端与进程组证据冲突必须拒绝。无锁且探测成功时可显示同 cwd、updatedAt 唯一的 recent 建议，须勾选确认；并列或探测错误不建议。其余 8 种 Agent 使用显式 session ID 启动参数；Kimi 另可使用欢迎界面的原生 ID。识别出的 ID 必须在该 Agent 同 cwd 的来源列表中。打开弹窗后来源切换、证据消失或对应 ID 不在列表时拒绝交接。使用真实 TUI 验证进程或屏幕证据、来源发现结果和窗口选中结果；只测唯一历史会话或 Shell 拒绝分支不足以验收。无明确证据的全新会话仍要求用户确认，不能用最新时间戳冒充当前会话。

脚本 CLI 入口验收：Gemini、Copilot 等前台进程名可为 `node`，应从同一进程的 argv 中识别实际 CLI 路径。非 Codex 的参数和 Kimi 欢迎界面只证明启动时的 ID，自动预选后须确认尚未在 TUI 内切换；`--continue` 之类没有明确 ID 的启动方式仍保留手选。

验收纠正：先在**仍运行的源 Agent Tab**点击 Handoff，Con 识别当前 Agent 并列出它在该目录的原生 session；唯一候选预选后由用户确认当前 ID，多项让用户选准确项。目标选项框随后可选择现有 Agent Tab 或新建 Tab。源 TUI 始终保持运行；源停止确认已于 2026-09-24 移除。测试必须覆盖同目录有两个来源会话、源 TUI 在选项框打开后退出或重启、目标 Tab 变化和源进程组变化；不得再显示“Exit the source Agent…”作为前置条件。

依赖顺序：T0 冻结本 ADR、架构契约及本计划；T1 实现目标 Tab 枚举、运行中来源绑定和精确会话选择；T2 接入现有 Tab 投递与新建 Tab 可见启动，并复用一次性 job 状态；T3 将面板改为直接选择目标，保留可展开证据与状态回执；T4 单线集成后运行单元、workspace 和真实 Con 交互验收。T0 完成后才能进入代码变更，T1/T2/T3 必须遵守文件边界并在 T4 汇合。

验收用例：来源 Agent 正在运行时点 Handoff，首先出现“已有 Agent Tab / 新建 Tab + Agent”选项；选择已有 Tab 不产生新 Agent 进程，且只向选中 Tab 投递；选择新建项产生新 Tab、以选中 Agent 启动。源 Agent 退出或更换、目标退出或 Tab 被替换、cwd 不同、同目录多来源会话未明确选择、重复点击、写入结果不明及快照变化都不能误发或自动重发。实际接续由目标读取包并给出回执确认；全过程保留源 Tab 与 Git 状态。

替代与风险：旧的“新 surface + 预览/粘贴”只保留为 CLI 兼容或失败恢复，不能充当新主路径。远程或无桌面环境无法完成 UI 验收时记录该限制，先运行核心测试，桌面可见时再验收 UI，不以无窗口测试冒充通过。

实现结果：入口核实源终端正运行受支持 Agent，并绑定当前 Tab、终端、前台进程组和目录；对话框列出同工作目录中正在运行受支持 Agent 的其他 Tab，也可选择新建 Tab 和已安装目标 Agent。来源会话唯一时预选并要求确认当前 ID，多会话时要求精确选择。执行前复核来源及目标的活性；投递前持久化不确定状态，新目标创建真正的 Tab。

技术验收：`cargo check -p con --bin con` 与 `cargo test --workspace` 通过；核心已有 Tab 路由 20 项 handoff 测试通过。窗口实际点击与目标 TUI 接收仍需有可见桌面的手动验收。本次未将非 Cursor 的新会话手动投递能力冒充自动投递；已有 Tab 走直接 PTY 投递后仍要求用户确认接续。

状态：本机 9 个 Agent 的来源与原生目标适配已实现；统一代码回归、安装能力探测和本机发现通过。
Con 窗口真实接续已验证 Codex→Cursor 自动投递和 Codex→Pi 手动投递；其余目标尚无逐个真实接续证据。
日期：2026-09-22；验收更新：2026-09-23。来源：[提案 #381](https://github.com/nowledge-co/con-terminal/issues/381)。

[设计 ADR](../design/agent-session-handoff.md) · [实际契约](../design/agent-session-handoff-contract.md)。

## 使用

构建 `cargo build -p con -p con-cli`，两个二进制必须相邻；macOS 发布包已有此布局。
通过 `con-cli handoff agents` 查看可用产品和诊断；每个目标使用自身正常登录。
探测 PATH 及本机常用安装目录，Cursor 只接受明确的 `cursor-agent`，不使用 `agent` 别名。

1. 保持源 Agent TUI 运行，在其 Tab 顶栏点击 Handoff，或从终端上下文菜单、命令面板选择 `Handoff Agent…`；也可运行 `con-cli handoff open`。
2. Con 识别当前 Agent，并列出它在该目录的原生会话；只有一个候选时确认显示的 ID 就是当前会话，有多个候选时选择准确的一项。
3. 选一个同项目中正在运行 Agent 的 Tab，或选择“New Tab”及目标 Agent，然后执行交接。
4. 已有 Tab 直接收到交接指令；新建路径创建真正的 Tab 并可见地启动目标原生界面。原 Tab 保留。
5. 目标读取交接包并复述目标后，按交接状态确认接续。Kimi 通过 PTY 自动投递并确认提交，失败才提示 Cmd-V；Codex 与满足版本门槛的 Cursor argv 自动投递。
6. 再次交接前，停止目标和后台命令并退出助手，再释放任务。

当前 Kimi New/Existing Tab 均为 PTY 自动投递，手动粘贴仅兜底。
原生 ID 启动前不可观测时，回执仅确认交接内容，不声称验证原生 session ID。
默认目标来自最后一条源用户请求；最新目标可覆盖旧要求，完整筛选历史可从任务卡片查看。

CLI 支持独立准备和恢复；将示例中的 ID/版本号替换为实际返回值：

```bash
con-cli handoff agents
con-cli handoff sources --agent codex --cwd /path/to/project
con-cli handoff prepare --cwd /path/to/project \
  --source-agent codex --target-agent cursor \
  --source-session <codex-session-id> \
  --goal 'Finish the remaining test failure; preserve existing work'
con-cli handoff get <handoff-id>
con-cli handoff start <handoff-id> --revision 1 --tab 1 --pane-id 0
con-cli handoff respond <handoff-id> --revision <current-revision> received
con-cli handoff cancel <handoff-id> --revision <current-revision>
# Only after stopping the target, background work and the visible helper:
con-cli handoff respond <handoff-id> --revision <current-revision> stopped
```

每次更改后读取返回的新 revision。start 需要 live Con；不要重用过期版本号。
状态结果不确定时检查已有目标，不再创建第二个会话。
隐藏的 `handoff run` 仅供 Con 新终端中的助手使用；它要求 TTY 和已经预留的任务。

## MVP 实施基线与模块（扩展前）

用户授权后先冻结 T0 类型/状态，再按适配、事务、UI/CLI、集成的顺序串行实施。
单负责人拥有新增模块和共享 dispatch，没有启动并行 Agent 或改动第三方源码。
所有新增代码文件小于 500 行。

| 模块                            | 已实现内容                                                         |
| ------------------------------- | ------------------------------------------------------------------ |
| con-agent/handoff               | 有界只读 Codex reader、精确会话选择、带来源记录、脱敏、Cursor 探测 |
| con-core/handoff                | 快照、不可变包、原子状态、版本冲突、job 状态、恢复和保留期       |
| con-cli/handoff                 | 公开命令、可见 create-chat/resume 助手、真实 TTY、无不确定重放     |
| con-app/workspace/agent_handoff | 菜单/命令面板入口、可搜索来源、预览、选项确认、新 surface、恢复    |
| 控制面                          | handoffs.open/sources/prepare/list/get/start/respond/cancel        |

UI 使用 gpui-component 的 Select、Input、Checkbox 和 Button；在后台处理导出、Git 和磁盘。
原生启动保留目标自己的模型、登录、信任和权限提示。
没有证据支持机器可读 TUI ACK，因此首版使用用户确认，不从屏幕稳定或启动成功推断完成。

## MVP 门 A：回归（扩展前）

全工作区 775 项测试通过（0 失败）；新增的锁继承测试加入后，交接核心的 13 项针对性测试全部通过。
严格 all-targets 编译、格式和 diff 检查通过。

最终运行：

```bash
cargo fmt --all -- --check
cargo test --workspace
RUSTFLAGS='-D warnings' cargo check --workspace --all-targets
```

针对性交接测试覆盖来源与最新纠正、隐藏记录过滤、分页/活动历史拒绝、参数编码、
请求幂等、跨重启 job 状态、错误来源、文件漂移、创建/投递崩溃窗口、路径与权限、
本地排除规则、显式回执、存活助手防释放、超时与助手竞争、目标 ID 保留、
终态保留期及继承描述符的显式解锁。
测试 fixtures 使用临时 Git 仓库，不读取用户的真实 Codex 存储。

macOS 本地编译不能代替 Windows/Linux CI；该功能在非 macOS 上拒绝执行。
仓库已有的 manifest 重复 binary 提示及依赖 future-incompatibility 提示仍存在。

## MVP 门 B：真实接续证据（扩展前）

工具版本：Codex `0.155.1`，Cursor `2026.09.18-9a7762b`。
Cursor 使用现有默认 `Grok 4.6 High Fast`，未覆盖模型或权限。
M0：create-chat → 指定 ID resume → 初始 prompt → 项目文件读取，返回唯一 sentinel。

主验收使用含中文和空格的临时仓库：

- Codex 只修复 `add()`；保留一个失败的 `multiply()` 测试。
- 保留一项暂存文件、一项未跟踪文件和未暂存的源修改。
- 使用明确源 ID `01a0c958-5c88-7670-9334-72c2b9f54c79`。
- 交接包包含 12 条筛选记录，context 为 8,395 UTF-8 字节。
- 目标 ID `5589f45e-e5f0-46bf-8489-b9ff87ae4afe` 在投递前持久化。
- 原生 Cursor 展示工作区信任提示，随后读取 context.md 和 evidence.json。
- Cursor 正确采用最新目标，覆盖历史“暂时保留失败”的旧要求，仅继续修复 `multiply()`。
- 独立运行 `python3 -m unittest test_calculator.py`：2 tests，OK。
- 独立核对 Git index 摘要、测试文件、暂存和未跟踪文件内容，均保留；无提交或额外暂存。
- 投递后保持 AwaitingConfirmation，显式确认才进入 Active。
- Cursor 以 exit 0 退出后确认测试 job 停止；未以取消请求代替真实停止。

### 2026-09-22 的 Con 窗口验收限制（历史记录）

2026-09-22 用户确认：验收期间 Mac 处于锁屏／远程无可见桌面状态。
当时这是已确认的环境限制；2026-09-23 已在获得实际布局帧的测试窗口中复测。

临时独立 Con 实例的控制 socket 正常；handoffs.open 返回 opened=true。
handoffs.start 保留源 surface 并创建带 owner 的独立目标 surface，落盘 LaunchPending。
但是目标 surface 始终未获得 GPUI 布局，surface_ready=false、没有 PTY。
原有 surfaces.create 在相同环境中也出现相同现象；直接二进制、临时 app bundle 和
LaunchServices 启动均复现。界面工具无法找到 CGWindow，返回 cgWindowNotFound。

因此上面的真实模型验收由同一个 con-cli 可见助手在独立 PTY 完成；
不能将其报告为 Con 窗口内从点击到接续的完整 E2E 已通过。
UI 已补齐激活 tab、布局通知、可见性同步和刷新；30 秒未启动且助手未占锁时给出恢复状态。
没有为了测试而用估算 bounds 初始化 NSView，或修改第三方代码。

上述结论只描述 2026-09-22 的锁屏环境，不代表 2026-09-23 的验收结果。

## 恢复、回退和后续

检查 `handoff get/list`；未知创建/投递状态不自动重发。准备后文件或源历史改变就重新准备。
记录保留在应用数据目录及项目私有暂存目录；终态 7 天后仅清理未被修改的登记文件。
关闭窗口或卸载功能不会删除 Agent 会话、回滚文件或杀死外部进程。

本机 9 个来源与目标之间可组合交接；不代表全部 81 种组合已做真实接续验收。
SSH、多 worktree 迁移、其他系统和未安装产品，继续遵循
[兼容性研究](../study/agent-handoff-compatibility.md)，不计入本批已实现范围。

## 本机多 Agent 扩展批次（2026-09-22）

用户最新要求：先实现本机已有 Agent 的适配，所有验证放到最后统一执行。
本批以本机 CLI 帮助、安装包代码和官方资料确定接口；这属于实现依据，不是交接验收。
新增阶段未完成前，前文 775/13 测试数字仅证明上一版，不代表本批已验证。

依赖：T0 类型契约 → {来源命令适配 A、来源文件适配 B、目标适配 C} → root 串行集成 → 统一验证。
使用 doc-driven-parallel-delivery 和 multi-agent-file-ownership-boundary 技能分配明确文件所有权。
每个 worker 在 /private/tmp/con-handoff-adapters-<role> 独立 worktree 编写新增文件，读取主目录契约，
不复制主目录未提交改动、不提交、不改其他 worker 文件；root 逐文件审阅并回灌新增适配模块。
用户的统一验证安排覆盖技能中的分波次自验：本批 worker 不运行编译/测试；所有模块完成后由 root 统一检查。

| ownership | 独占路径 | 工作与交付 |
| --- | --- | --- |
| root | 共享 types/mod/protocol、core、CLI、UI、文档 | 类型、通用调度、向后兼容、界面选择、串行集成和最终统一验证 |
| sources_cli | con-agent/src/handoff/sources/{opencode,dimagent,grok,cursor,acp,cli_support}.rs | 4 个来源 reader；实际原生导出或只读 ACP 回放（已移除适配的历史路径。） |
| sources_files | con-agent/src/handoff/sources/{pi,gemini,copilot,kimi,kimi_wire,file_support}.rs | 4 个来源 reader；有界、精确 ID、版本化文本读取；不含全局日志 |
| targets | con-agent/src/handoff/{target,inventory,launch}.rs | 9 个目标能力、原生新会话和 argv；安装发现与保守自动投递资格 |

共享文件禁止 worker 修改；有额外依赖或无法证明的接口就报告 root，不扩大授权或伪造能力。
代码文件小于 500 行。来源与目标分别标注实现状态；锁屏环境下图形 E2E 仍留到可见桌面后统一验收。

### 扩展实现矩阵

下表是代码实现与格式依据，真实目标接续统一留待可见桌面验收。

| Agent（本机版本） | 来源读取与边界 | 目标新会话 |
| --- | --- | --- |
| Codex 0.155.1 | app-server list/read；只读、拒绝活动/分页历史 | 原生新建，启动前 ID 不可观测 |
| Cursor 2026.09.18-9a7762b | ACP list/load；只读回放，标注部分覆盖，拒绝权限/工具请求 | create-chat 后指定 ID resume；沿用精确版本自动提示 |
| OpenCode 1.18.27 | 原生 JSON session list/export；准确 ID 和目录，未解决 revert 拒绝 | 原生新建，ID 不可观测；手动投递 |
| Kimi 2.1.1 | state + main wire，协议 1.0–1.5；恢复分支/undo，拒绝未结束 turn | 原生新建；PTY 自动投递（桌面验收待完成） |
| Pi 0.86.1 | JSONL v2/v3 的持久化末端分支与压缩；自定义 session-dir 暂不枚举 | 预分配 session UUID；手动投递 |
| Gemini 0.60.0 | JSON/JSONL、rewind/set；必须有明确项目注册或目录标记 | 预分配 session UUID；手动投递 |
| Copilot 1.0.80 | workspace.yaml + events；仅最终连续 cwd 段，未知 rewind 拒绝 | 预分配 session UUID；手动投递 |
| Grok 1.0.40 | summary + ACP updates 文本；无法重建的 rewind 拒绝 | 预分配 session UUID；手动投递 |
| DimAgent（已移除；历史） 0.3.26 | 版本限定的 SQLite/WAL 私有快照；只取会话/消息白名单 | 原生新建，ID 不可观测；手动投递 |

未安装的 Claude Code、Qwen、Goose、Amp、Droid 等未添加可用适配，不安装新 Agent。
新类型保留旧任务默认值；来源/目标 Agent 纳入幂等和漂移核对；无 ID 启动也禁止不确定重试。
CLI 新增 agents、sources --agent、prepare --source-agent/--target-agent；控制面新增 handoffs.agents。
UI 同时选择来源和目标，只显示已探测兼容的产品，来源会话始终显式选择。

实现期仅读取帮助、安装源码和协议文档。DimAgent（已移除；历史） `tui --help` 曾意外进入 TUI，
当即终止本次精确进程，未输入提示；正式探测只调用顶层 --help/version，禁用 wrapper 下载。
这一发现用于规避启动副作用，不计作接续验收。

### 扩展统一验证记录

所有适配完成后才开始验证，未启动任何新增目标的模型接续。

- 首轮完整 `cargo test --workspace`：790 passed，0 failed。收尾改动后统一重跑 `cargo test -p con-agent -p con-core handoff::`：来源/目标 20 项、核心 17 项全通过（包含前一轮已有测试，不能与 790 简单相加）。
- 最终代码的 `RUSTFLAGS='-D warnings' cargo check --workspace --all-targets`、格式与 diff 检查通过；新增 37 个交接代码文件最大 425 行。
- 真实本机 `handoff sources --agent … --cwd <本仓库>`：9 种全部成功；结果只核对类型与数量，未输出会话正文。
  Codex 16、Cursor 1、OpenCode 10、Kimi 7、Pi 8、Gemini 0、Copilot 0、Grok 1、DimAgent（已移除；历史） 0。
  空列表仅说明当前目录无匹配记录，不算该 Agent 的真实历史导出已验收。
- `handoff agents` 最终 9/9 产品均通过来源与目标能力探测。OpenCode 将成功 help 全写到 stderr；改为仅该产品并发、有界读取 stdout/stderr 后复测通过。
- 统一静态审查发现并修复 Delivering 可提前确认接续的问题，17 项核心测试通过。
- DimAgent（已移除；历史） 的临时真实 SQLite/WAL fixture 通过：副本可查询但不可写，原 DB/WAL 字节不变，原库变化被拒绝，临时副本自动清理。
- 截至 2026-09-22 仍未验收：新增来源的真实停止后全文导出与目标接续、手动粘贴和登录/权限提示、Con 可见 surface。
  用户当时确认锁屏／远程无桌面；后续结果见下节。

回归测试仅使用本地临时 fixture；本机会话发现仅输出计数。安装探测不授予权限、不改变模型、不创建新目标。

### 2026-09-23 可见窗口验收

使用独立测试版 Con app bundle 和独立控制 socket；原有 Beta 进程未参与。
测试仓库 `/private/tmp/con-handoff-live-jc3rw4pt` 预置一个缺失的 `multiply()`、失败的单测，
以及暂存、未暂存、未跟踪三种哨兵改动。真实窗口获得布局帧，新增 surface 有 PTY 且 `surface_ready=true`。

- UI 显示 9 个来源和目标；精确会话 ID 移到标题前，长标题不再遮蔽 ID。完整上下文固定在可滚动的预览区域，底部操作按钮始终可见。Prepared 任务经重启可恢复；编辑目标会释放旧预览并重新准备。
- Codex 来源会话 `01a0cc05-c8cb-71a0-8c6b-c21fd6d446cc` → Cursor 目标会话 `32bd2924-e23e-4fca-b824-597397b921fe`：在 Con 新 surface 内自动投递，原生工作区信任提示可见。Cursor 读取 `context.md` 和 `evidence.json`、复述 handoff ID 和最新目标，只补充 `multiply()`。自身运行 2 项测试通过；独立复跑亦为 2 tests OK。原有三种 Git 改动内容不变，HEAD 不变，也没有额外暂存。任务在显式确认前保持 AwaitingConfirmation，确认后进入 Active，目标结束后按停止确认结束 job。
- Codex → Pi 目标会话 `15a15ca4-3416-4284-be7d-998188730933`：在 Con 新 surface 内手动复制、粘贴并发送指令；“I sent it” 只在实际投递后点击。Pi 读取上下文、报告 handoff ID、复跑 2 项测试并确认三种哨兵改动；未改文件。确认接续后退出 Pi，助手返回 shell，再结束 job。
- 新发现的来源缺口：Cursor 2026.09.18 CLI 会话在 `~/.cursor/chats` 和 `agent-transcripts` 中持久化，但 ACP `session/list` 返回空。加入有界、只读的本地 CLI transcript 读取作为补充；只导出已完成 turn 的 user/assistant 文本，省略 thinking、工具参数与不透明内容。实测刚创建的 Cursor 会话现在可发现并成功导出 5 条筛选记录；`evidence.json` 不含私有 thinking/signature。Pi 刚创建的真实会话也可发现并导出。两次来源导出只准备预览，未启动目标，随后取消 job。
- 9/9 安装产品仍通过来源和目标能力探测；对测试仓库的 9 种来源发现命令均正常返回，其中 Codex、Cursor、Pi 各 1 条，其他 6 种没有该目录的会话。空列表不等于真实全文导出通过。

本次真实接续证明自动投递和手动投递两条代表性路径。OpenCode、Kimi、Gemini、Copilot、Grok、DimAgent（已移除；历史） 等目标尚未逐个登录、手动投递和验证原生回执；不声称全部 81 种来源→目标组合通过。锁屏时的无布局帧问题在本次可见窗口未复现，不能据此证明锁屏环境可用。

## 目标模型选择扩展（2026-09-24）

需求：「新建目标 Tab」时除 Agent 类型外允许**选择模型**（可选，默认不指定）。调研结论：
9 种本机 CLI 中 8 种支持启动时 `-m/--model` 指定模型（DimAgent（已移除；历史） 待验证）；模型列表能力
差异极大、无法统一探测，因此采用「精确 ID 输入 + 部分 Agent 提供候选」。契约中原有的
“不添加模型覆盖”禁令已在《架构契约》“模型覆盖约定（2026-09-24）”一节修订，本批以此为准。

依赖顺序：**T0 契约冻结（本批）→ A 适配层 / B UI / C 助手与测试 三路并行 → root 单线集成收口 → 统一验收**。
A/B/C 必须等 T0 合入后开始，并严格遵守文件边界。

| 批次 | 独占路径 | 交付 |
| --- | --- | --- |
| T0 | `docs/design/agent-session-handoff-contract.md`、`docs/impl/agent-session-handoff-plan.md`、`crates/con-core/src/handoff/types.rs`、`coordinator.rs`、`tests.rs`（本批已落地） | 契约修订；`PrepareRequest.target_model`、`validate_target_model`、`MAX_TARGET_MODEL_LEN`、`LAUNCH_HELPER_PROTOCOL`；幂等比较核对；全部构造点编译通过 |
| A（适配层） | `crates/con-agent/src/handoff/{launch,target,inventory}.rs` 及 con-agent 侧测试 | 各产品模型旗标探测（含 DimAgent（已移除；历史） 验证）、`target_args` 追加独立 argv 模型参数、版本/旗标不支持时的显式错误；不得改 shell 拼接语义 |
| B（UI） | `crates/con-app/src/workspace/agent_handoff/`（destination 面板等） | 「新建 Tab」路径的模型输入与候选展示、「已有 Tab」路径不暴露模型选择、调用 `validate_target_model` 预校验、「请求使用模型」措辞 |
| C（助手与测试） | `crates/con-cli/src/handoff.rs`、`crates/con-test/testdata/` 相关用例 | prepare CLI 增加模型参数；`handoff run` 助手实现 `LAUNCH_HELPER_PROTOCOL` 核对、不认识模型字段必须拒绝启动；集成测试覆盖新旧助手混用拒绝分支 |

共享文件边界：A 不改 con-core / con-app / con-cli；B 不改 con-agent / con-core（types.rs
新增导出可直接消费）；C 不改 con-agent / con-app。任何一方需要越过边界时报告 root 收口。

T0 已为 A/B/C 冻结的接口：

- `PrepareRequest.target_model: Option<String>`（`#[serde(default)]`，参与 request_id 幂等比较）；
- `con_core::handoff::validate_target_model(&str) -> Result<()>` 与 `MAX_TARGET_MODEL_LEN`；
- `con_core::handoff::LAUNCH_HELPER_PROTOCOL = 2`（C 实现握手，Con 派生前与助手读取 job 后双重核对）；
- 失败语义约定：发送前模型失效或旗标不支持 → 重新准备，绝不静默回退默认模型；启动后目标判定无效 → `NeedsInteraction`。

验收标准（本扩展整体，单线集成后执行）：

1. 未选模型时新建 Tab 的 argv 与扩展前逐字节一致；旧 job（无 `target_model`）读取与启动行为不变。
2. 选定模型时目标 CLI 以独立 argv 收到模型参数；`validate_target_model` 拒绝空值、控制字符、超长、`-` 开头值，且非法值在持久化前被拒绝。
3. 「已有 Tab」路径构造的请求 `target_model` 恒为空，UI 不出现模型入口。
4. 同一 request_id 变更模型报冲突而非重放；旧 con-cli 助手遇到模型任务拒绝启动而非按默认模型执行。
5. UI 文案为「请求使用模型」，不承诺实际生效模型。
6. `RUSTFLAGS='-D warnings' cargo check --workspace --all-targets`、`cargo fmt --all -- --check`、`cargo test --workspace` 通过；可见桌面下至少完成一种「选定模型的新建 Tab 交接」真实验收（验收所属目标以 A 的探测结果为准），不验收的组合如实记录。

## 支持范围收缩（2026-09-24）

用户产品决策：移除 OpenCode、Pi、Gemini、GitHub Copilot、Grok 的 handoff 支持，
保留 Codex、Cursor、Kimi、DimAgent（已移除；历史）。

- **类型层。** `AgentKind` 由 9 个变体收缩为 4 个；`ALL`、序列化名称、label 与
  可执行名同步更新。
- **来源侧。** 删除 `sources/{opencode,pi,gemini,copilot,grok}.rs` 及仅被它们使用的
  `file_support::header`；`discover_sessions`/`export_session` 分支收缩。
- **目标侧。** 删除这 5 个产品的 help/version 品牌校验、argv 分支、预分配 UUID 分支与
  Grok 候选模型探测；`.grok/bin` 从子进程 PATH 回退目录移除。
- **进程绑定。** `agent_from_executable` 映射与 `explicit_session_arg` 参数表收缩。
- **UI/CLI。** 目标图标映射收缩为 4 种；CLI 的 `--agent/--source-agent/--target-agent`
  由 `AgentKind` 解析自动收缩，其他名称报 `Unknown agent`。
- **兼容性（2026-09-25 修订）。** 旧 job / bundle 中被移除或未知的 Agent 名称读取为
  `AgentKind::Unknown`，不加入 `ALL` 或 CLI 可选项；来源读取、目标探测/启动与投递均拒绝。
  保留原 job 状态，不因无法识别 Agent 就标为 Failed：旧目标可能仍在运行。
  相关记录可在当前卡片或 CLI 检查、取消；未确认投递可显式放弃，已有回执须确认停止。
  旧记录与 corrupt 目录均不阻塞新 prepare；仍保留诊断告警。
  其他损坏记录仍按既有不可读取规则保留。

验证（2026-09-24）：`cargo test --workspace` 856 passed / 0 failed；
`cargo check -p con --all-targets`、`cargo fmt --all -- --check` 与
`RUSTFLAGS='-D warnings' cargo check --workspace --all-targets` 通过。

## 移除发送前预览确认页（2026-09-24）

用户产品决策：删除 REVIEW & SEND 复核页，主表单的 **Send handoff** 一次点击即
prepare 并直接进入路由投递，中间不再有第二次确认，也没有 Discard。

- **UI。** 移除 `PendingPreview`、`render_pending`、REVIEW & SEND 分支与
  Discard/Send 页脚；主表单页脚按钮由 `Prepare preview` 改为 `Send handoff`，
  流程变为 `confirm → execute → finish_prepare → dispatch_prepared`。
- **状态。** `finish_prepare` 成功后直接 `cx.emit(ExecuteHandoff)`，面板保持 busy：
  成功关闭对话框，失败由 `report_error` 回到任务卡片。`send_pending` 与
  `discard_pending` 合并为 `dispatch_prepared`；两者之间的模型漂移复核随之取消，
  因为 prepare 与发送之间已无用户操作窗口，模型值仍由 `validate_target_model_for_agent`
  在持久化前校验。
- **材料可达性。** 预览页提供的 `evidence.json` 入口迁移到任务卡片的 “View evidence”，
  与 “View handoff context” 并列，能力不因移除页面而丢失。
- **文案。** 指向已移除预览页的错误文案改为 prepare 语义：con-core 的
  `Workspace changed; cancel this handoff and prepare again`、
  `Source history changed after prepare; …`、`Target agent changed after prepare; …`，
  `route/mod.rs` 的分类用例同步。
- **保留不变。** prepare 后的 bundle 完整性读取、来源会话重绑、工作树快照校验、
  模型值校验、关窗取消未启动任务（`preparing_request_id`）、投递后状态机与
  手动投递的 “I sent the instruction” 全部不变。

契约见 [架构契约](../design/agent-session-handoff-contract.md) 的“投递确认语义”一节。

验证（2026-09-24）：`cargo test --workspace` 856 passed / 0 failed；
`cargo check -p con --all-targets`、`cargo fmt --all -- --check` 与
`RUSTFLAGS='-D warnings' cargo check --workspace --all-targets` 通过。

## 审查收口 S4–S10（2026-09-25）

- Handoff 面板每次打开强制刷新安装能力；CLI / 后台查询仍复用 300 秒缓存。
  刷新不读取会话正文，不启动目标；新装或升级 CLI 无须等待 TTL。
- 自动会话识别失败时显示非阻塞提示，继续允许手动选择及确认。
  prepare 后的复核区分会话变化与检查失败；两者均取消未投递任务，检查失败记录原始原因。
- DimAgent（已移除；历史） 来源当时仅支持 `dimagent 0.3.26` 的私有 SQLite/WAL schema。
  UI 分开标注来源导出能力与目标可用性；来源版本不支持不禁用已验证的目标启动。
  更新来源适配时见下方[适配更新流程](#适配更新流程)。

### 平台限制

Agent Handoff 为 **macOS-only**。非 macOS 的 prepare 由
`HandoffService::prepare` 中 `ensure!(cfg!(target_os = "macos"))` 在运行时拒绝；
`agent_from_process_argv` 在非 macOS 返回 `None`。当前未做 feature gate 编译期隔离，
部分共享 handoff 模块仍编入 Windows/Linux workspace。
**Handoff 的 CI 验收以 macOS job 为准**；其他平台 workspace 构建通过不代表 handoff 可用
或其 macOS 进程绑定路径得到验证。短期保留此 documented limitation，不扩大平台支持声明。

### 旧适配更新流程（历史，已移除）

DimAgent（已移除；历史） 升级来源支持前，核对新版本 schema 与完成 turn 的语义，更新
`con-agent/src/handoff/sources/dimagent.rs` 的只读快照 reader 和（已移除适配的历史路径。）
`inventory.rs` 的来源版本能力判断；用该版本 SQLite/WAL fixture 验证只读、变更拒绝、
字段白名单与导出完成状态，再更新版本约束。单独验证目标顶层 help/version 与原生启动，
不能因来源可读就宣称目标自动投递可用，也不能因来源不支持就标记目标不可用。


### 本轮验证

2026-09-25：`mise exec -- cargo build --workspace`、
`mise exec -- cargo clippy --workspace --all-targets -- -D warnings`、
`mise exec -- just lint` 均通过；`mise exec -- cargo test --workspace`
为 **873 passed / 0 failed**（基线 866，新增 7 项）。新增覆盖复核错误/漂移两项、
缓存刷新一项、未知 Agent 能力拒绝一项、旧数据读取与取消两项、面板禁止未知目标自动释放一项。
本轮未做可见桌面 UI/真实目标接续验收；未做非 macOS 编译隔离。

## 四项实测回归修复（2026-09-25）

按 P4 → P3 → P2 → P1 修复；诊断根因成立，保留 a3441bca 环境白名单。
来源和投递边界见架构契约“实测回归修订”，复盘见
[postmortem](../../postmortem/2026-09-25-handoff-source-and-delivery-regressions.md)。

- Cursor：argv ID 优先；缺失时复用本地 discover，以 cwd + 最近唯一时间提供需确认的建议。
  无 meta、时间未知或并列不绑定；未改变完整性复核。
- Kimi：明确进程名与解释器屏幕识别；continue 不解析为 ID，空 banner 不再隐藏历史。
- 模型：Kimi provider JSON 只提取 model key，使用对应 Agent 白名单环境，不输出原文。
- 投递：Codex 开启原生 argv prompt；Cursor 放开至 2026.09.18 及以后且通过 help 检查。
  Kimi 保持交互 TUI，Con 注入前静默 pbcopy 备份，失败保留卡片复制入口；DimAgent 已移除。

本机 Kimi 2.1.1：`provider list --json` 返回 4 个模型 key；执行前后检查的 123 个
配置类 JSON/TOML 文件内容未变，原始 stdout 未显示或保存。`--help` 的 -p 明确为
非交互一次性执行；DimAgent（已移除；历史） 顶层帮助的 exec 也是 one-shot，不作为交互首次提示接口。
Cursor 本机帮助确认 positional initial prompt，Codex 帮助确认交互 [PROMPT]。

最终验证均以 `mise exec --` 执行：`cargo build --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`just lint` 成功；
`cargo test --workspace` **884 passed / 0 failed**，较 873 基线新增 **11** 项。
修改/新增 Rust 文件均小于 500 行。未做真实桌面多 Agent TUI 接续验收；
Kimi New/Existing Tab 的 PTY 自动提交及失败 Cmd-V 兜底、其他目标的 raw-mode 接续仍须逐产品桌面验收。

## 第二轮回归与三 Agent 范围（2026-09-25）

按 B → C → A → D 串行实施，不修改诊断证据。
B：过滤空环境值、防御性保留 assist endpoint；非零退出记录键名诊断并进入 NeedsInteraction。
原始 bootstrap timeout 未复现，尚需用户环境复验。
C：仅读模型缓存文件的 fetched_at/slug/visibility，600 秒缓存；旧候选标记 degraded，错误降级手输。
A：所有 dispatch 都校验相邻 helper 协议 ≥2；Kimi 提交确认后卡片显示 Instruction sent to Kimi；失败才提示 Cmd-V。
D：删除第四种适配；当前仅 Codex、Cursor、Kimi。旧名称按 Unknown 保留 job并允许显式处理。
构建须使用同版本相邻 Con/con-cli：`mise exec -- cargo build -p con -p con-cli`。

## 2026-09-25 第五轮诊断修复

- P0：Kimi 专用 raw write 通过 Ghostty `text:` binding 单次入队，不改变键盘和 argv。
- P1：未确认交接增加 Abandon；新 Kimi Tab Delivering 可一步取消，迟到错误不能复活已取消 job。
- P1：移除历史聚合 UI/删除逻辑；当前仅展示最新相关非终态 job，与发送表单并存。
  round7 按用户裁定删除 prepare 冲突及其恢复动作，同 worktree 默认允许多个 job。
- P2：Codex 区分无 writer lock、多锁、探测失败，仍必须手选真实当前会话。
- 验证：mock 字节 sink、marker 匹配、任意旧 job 状态下同 git_dir 默认可 prepare、提交回执禁用
  abandon、迟到错误不复活；完整 workspace build/test/clippy 与 just lint。
  native Kimi 与登录 turn 的验收边界见 `agent-session-handoff-kimi-validation.md`。

## 2026-09-25 第七轮：移除 worktree 租约

执行蓝本为本地 round7 诊断；用户“太重”裁定推翻保留建议。六步分别提交：
1. 删除 git_dir / corrupt prepare gate 与租约类型；幂等、Unknown 兼容和记录保留不变。
2. 目标进程 reconciliation 改名 presence / cancel_absent_target，保留卡片自动 Cancelled。
3. 删除 lease_job、冲突 UI、Abandon 后重发，合并 tracking；显示最新相关 job 与 Send 表单。
4. Abandon/cancel 只作用这一 job，快照与历史漂移提示新建 job，无须先清理旧任务。
5. Existing Tab 的 Delivering / LaunchPending 阻止同 Tab 连发；route 使用服务层原子检查，
   同一个 store 锁涵盖检查与状态写入，避免分离检查导致双写 PTY。新增真实竞争和状态边界两测试。
6. 修订契约、设计、验收与 postmortem；完整 workspace 验证。

每次 Send 新建 request_id；同一 worktree 多 job 并行，单 job revision 与状态机防重放。
失败 job 不挡新 prepare，目标进程可能仍活着；多 Agent 写文件风险由用户接受。
Cancelled/Failed 7 天清理、launch.lock、助手协议、argv/PTY 回执与 Codex 绑定不改。
验证详情与测试删改见 [本轮记录](../../postmortem/2026-09-25-handoff-worktree-lease-removal.md)。
