# Agent Handoff：架构契约

## 实测回归修订（2026-09-25）

本节覆盖下方旧版“禁止最近会话建议”和 Cursor 精确单版本自动投递限制。
Cursor 缺少 argv ID 时复用本地 meta/transcript discover，按来源 Tab 的规范化 cwd
筛选最近更新时间唯一的可导出会话；这是“最近会话建议”，必须用户确认，
时间未知或并列则不绑定。Codex 在进程组和 app-server 对端探测均成功但无证据时，
同样允许同 cwd、updatedAt 唯一的 recent 建议，`requires_confirmation: true`；
探测失败或证据冲突不得进入 recent。发送前后继续复核，不能将建议宣称为进程持有证据。
Kimi 的 `-c/--continue` 是布尔参数，不是 ID；空 Session 横幅仅禁用自动选择，
磁盘历史仍可手选。屏幕识别仅补充 node/bun/python/python3 宿主，shell/editor 不接受，
缓存品牌与直接识别冲突仍拒绝；`Kimi Code` 是明确进程名。

Codex 通过现有 help 能力检查后使用 `-- prompt`；Cursor 通过 help 检查且版本日期
不早于已验收的 2026.09.18 时沿用 positional prompt。日期门槛基于已验收接口，
不声称未来版本或本机新版本已经做过桌面接续验收。
Kimi 2.1.1 `-p` 是非交互模式，不用于 handoff。New/Existing Tab 均由 Con
读屏就绪后发送 bracketed paste + newline，观察提交后才记录
`pty_injection_submit_observed` 并进入 Active。注入前以白名单环境静默 pbcopy
短 instruction 作备份；原始上下文不复制。Trust this folder / Don't trust 选择态禁止注入。
未就绪、写入失败、提交未确认均进入 NeedsInteraction，卡片提供 Cmd-V 兜底和显式确认。
首次载荷中的 LF 在本次隔离 2.1.1 PTY 实验中停留于编辑器；至少 2 秒后，仅在读屏
证明同一完整 instruction 仍在输入框时补发一次 CR，不重复粘贴未知结果。
实录、回归测试与桌面验收见 `docs/impl/agent-session-handoff-kimi-validation.md`。
Codex/Cursor Existing Tab 的写入回执仍仅证明 PTY 接受，不证明提交。

Codex 模型候选仅从 `{CODEX_HOME|~/.codex}/models_cache.json` 读取：
反序列化字段仅 `fetched_at`、`models[].slug/visibility`，不读取认证或配置文件。
复用 600 秒内存缓存；旧文件候选继续作为尽力提示并在 debug 日志标记 degraded，
不刷新网络、不启动子进程；缺文件或坏 JSON 降级手输。

Kimi 模型探测使用只读 `provider list --json`，只提取顶层 models 的 key 并过滤；
provider 配置、凭据、endpoint 和原始 stdout 不进入 UI、日志或错误诊断。

## 启动防御与投递提示（2026-09-25）

- 每次 dispatch 前校验 Con 相邻的 con-cli 协议 ≥2，包括未选模型及已有 Tab；
  缺失、不匹配或无法查询均明确拒绝，提示同版本重建 Con 与 con-cli，不从 PATH 回退。
- 环境白名单过滤空值；Codex 防御性允许 CODE_ASSIST_ENDPOINT。
  原 account/read bootstrap timeout 未在本机复现，不能据此声明修好原错误。
- 目标非零退出进入 NeedsInteraction；记录 agent、版本及 launch env 键名集合，不含值。
  原始错误保留在目标 TTY；失败记录不因进程退出自动释放，用户检查后显式处理。
- Con 在 Kimi 提交确认后通过非模态任务卡片显示 Instruction sent to Kimi；失败才提示 Cmd-V；
  仅 dispatch 或 Delivering 状态不表示剪贴板已就绪。启动观察窗口为 30 秒，
  后续失败仍保存在任务卡片。Codex argv 自动投递保持不变。

## Tab 路由契约（2026-09-23）

当前会话识别：在 macOS 上，以前台进程组 leader PID 为查询对象。Codex 读取该进程及其 localhost ESTABLISHED TCP 连接的 `codex app-server` 对端进程打开的文件（0.157 的 managed daemon 可位于不同 pgid）：优先 `CODEX_HOME/sessions/**/rollout-*-<UUID>.jsonl`（仅当前正在写入的线程持有，为强证据），无 rollout 证据时回退到 `thread-writer-locks/<UUID>.lock`；leader 两类证据都缺失时扫描其前台进程组其他成员（兼容包装启动器）。按完整反向 TCP 地址/端口对匹配对端，并核验 app-server argv 和进程身份；不得仅按监听端口或全局 daemon 名称绑定。libproc 空结果继续 lsof 兜底，失败保持错误。每个进程组/peer 内保留 rollout 优先；各来源解析出的 ID 冲突时拒绝绑定。同一类证据出现多个不同 ID 时不作选择。Cursor、Kimi 只解析各自原生 CLI 的明确 session ID 启动参数。Kimi 欢迎界面同时显示原生 ID，可作为屏幕证据。仅当证据唯一，且与本机 adapter 在当前 cwd 返回的 `SourceSession` 精确一致时，UI 自动选中。Codex 的打开文件证据（rollout 优先、writer 锁回退）可直接验证；启动参数和 Kimi 欢迎界面只证明启动时的 ID，TUI 可能在同一进程中切换会话，因此须用户确认它仍为当前会话。进程和界面给出冲突 ID、参数只表示 `--continue`/最近会话、空会话尚未持久化、ID 不在列表时均不得选最近会话代替；只有成功探测但没有进程证据时允许上述 recent 建议与勾选确认。准备前后复核来源 ID；路由时复核 Tab、terminal、进程组及 cwd。

修正契约：`open` 必须看到本地、存活、前台受支持 Agent。入口同步绑定来源 Tab ID、terminal EntityId、前台进程组、AgentKind、cwd；来源仍为 TUI 运行中。会话发现仅查询这个 Agent 和 cwd；有精确证据时自动选中，无证据时唯一项预选并要求用户确认，多项要求用户显式选原生 ID。执行前复核相同 Tab、terminal、进程组、AgentKind、cwd；任一变化拒绝发送。退出 TUI 后不能以缓存 Logo 发起新交接；既有 handoff 仍可通过 CLI 查看或释放。

此修正覆盖下方“只有原 Agent 及后台命令停止后才能准备”中关于进程退出的任何解释；`source_stopped` 与源停止确认已按 2026-09-24 用户产品决策移除，见“源停止确认的移除”一节。替代方案“从 Shell 回查会话”不可精确绑定。风险：某些运行时未暴露当前原生 ID；多会话时交给用户明确指定，不从更新时间推断。

- UI 入口绑定点击时的来源 `tab.summary_id`、terminal `EntityId`、前台进程组和规范化 cwd；源 Agent TUI 保持运行，交接不要求源停止确认。来源会话 ID 仍须来自对应 adapter 的精确导出；有多项时明确选择。
- 目标选项是 `Existing { tab_id, terminal_id, agent, cwd, foreground_group }` 或 `New { agent }`。现有项只包括其他 Tab 内**当前运行**的受支持 Agent；不能仅凭缓存图标、终端标题或滚屏证明活性。选择和发送时均核对 Tab、terminal、cwd、前台进程及 Agent 身份。失配直接报错，不降级为 Shell 投递。
- 新建项必须创建真正的 Tab 并在其终端可见地运行目标；目标类型来自安装能力探测。不得再在源 Pane 中建 surface 充当新 Tab。
- 两条路径共用不可变交接包、Git 快照和每 job 独立事务；`Prepared` 后只接受一次交接执行。向现有 Tab 写入前须持久化 `Delivering`，写入被观察到后直接 `Active`（用户 Send 即最终确认，见 2026-09-24 决策），写入结果未知时留在 `Delivering` 待人工核对，不自动重发。既有原生会话 ID 未经验证时保持 `None`。
- 现有 Tab 路径调用 `begin_existing_delivery(id, revision, tab_id)`，同一事务中复核快照、暂存交接包并记录 `existing_target_tab_id` 与 `Delivering`。只有 PTY 写入被观察到后才调用 `record_existing_delivery`；该调用把任务置为 `Active` 并记录“写入已观察”回执，仍不代表目标理解。崩溃后仍在 `Delivering` 时，用户看见目标已接续可显式 `confirm_received`，也可停止目标后释放；系统不得自动重发。
- 状态 UI 分清“已投递”和“已接续”；源 Tab 不被清除，目标退出与 job 取消遵循下文恢复规则。

决策：不要求每个目标 Agent 支持私有会话文件改写或统一协议输入；使用原生 TUI 的可见输入。替代的后台无头 Agent 会破坏 Tab 接续语义。风险是 TUI 输入框忙碌或权限弹窗时无法可靠确认，因此不自动重放，必要时进入需用户核对的状态。

状态：2026-09-25 起支持本机 3 个 Agent（Codex、Cursor、Kimi）的来源与原生目标适配；
OpenCode、Pi、Gemini、GitHub Copilot、Grok 的适配已按用户产品决策移除，见“支持范围的收缩”一节；
统一验证见实施记录。
本版取代 MVP 固定 Codex → Cursor 的范围；不实现自动 ACK 或自动中断。
[设计 ADR](agent-session-handoff.md) · [实现、操作与验收](../impl/agent-session-handoff-plan.md)。

## 职责

| 模块                                | 职责                                                             |
| ----------------------------------- | ---------------------------------------------------------------- |
| `con-agent::handoff`                | 3 个来源 reader、带来源的文本导出、敏感行过滤、安装能力与原生 argv |
| `con-core::handoff`                 | Git 快照、不可变交接包、事务状态、幂等、job 状态和清理         |
| `con-app::workspace::agent_handoff` | 原生终端绑定、来源与目标选择、可见启动及恢复                     |
| `con-cli handoff`                   | 公开操作接口、继承 TTY 的启动助手                                |

依赖方向保持 `con-core → con-agent`；con-agent 不依赖终端或 UI。
磁盘和协议操作离开 GPUI 线程，UI 复用 harness 的 Tokio runtime。
外部 Agent 保留自己的登录、模型与审批策略，不能作为内置 Rig 的 provider 处理。

## 导出与身份

`SourceSession` 包含 `agent/id/store_identity/title/cwd/updated_at`。
存储身份由规范化存储目录和主机名的摘要组成，不含凭据；另核对 Agent 类型。
Codex 使用 app-server read；Cursor 用 ACP load 只读回放；Kimi
读取已核对的本机持久化格式，具体覆盖范围见实施记录。
Cursor load 可初始化会话服务，拒绝工具、文件、终端和权限请求，不请求推理或认证。
列表最多 2,000 条，单次导出最多 10 MiB，协议请求超时 30 秒。
按用户选定的原生 ID 读取，并验证返回 ID 和规范化目录一致。

`HistoryExport` 包含源身份、Agent 版本、最后持久化位置、记录、缺失说明与摘要。
记录保留 `turn_id/item_id/role/text`，角色为 user、assistant 或 tool。
原生 ID 不可用时使用稳定的历史位置 ID，并明确其并非原生 turn ID。
保留可归属的文字请求、回答和历史工具输出；不导出隐藏推理、不透明工具参数及非文字输入。
按产品重建持久化分支、撤销或压缩记录；无法证明完整覆盖时标注，无法安全恢复时拒绝。
拒绝空目标历史，已知活动 turn/tool 也拒绝；导出是只读回放，不代表源 TUI 或后台命令已停止。

对潜在凭据行、私钥块和控制字符进行过滤。模式过滤无法保证识别所有秘密；
完整筛选历史（context.md、evidence.json）供用户从任务卡片检查。目标权限不能从历史中的授权继承。

## 包与快照

`PrepareRequest`：`request_id/cwd/source_agent/target_agent/source_session_id/goal/target_model`。
request_id 是规范 UUID。goal 最多 4 KiB。
`target_model` 可选（默认不指定），约束见“模型覆盖约定”一节。
相同 ID 和相同输入返回原任务；相同 ID 的不同输入报冲突——`target_model` 参与该比较，
同一 request_id 换模型是冲突而非重放。

`HandoffBundle`：`schema_version=1/handoff_id/created_at/history/workspace/goal/context`。
上下文最多 32 KiB；当前目标、最初/最近用户请求、近期进度和测试输出为确定性摘录，
完整筛选记录在 evidence.json。没有额外摘要模型调用。

`WorkspaceSnapshot`：`cwd/root/git_dir/git_common_dir/head/index_digest/worktree_digest/status/untracked`。
index 是 `git ls-files --stage -z` 的摘要。对 tracked 和非 ignored 的 untracked 文件
计算内容、权限和符号链接目标摘要；不修改索引，不提交，不重置，不复制工作树。
拒绝子模块、非 UTF-8 文件名、目录逃逸和特殊文件；限 100,000 文件、单文件 64 MiB、
总计 1 GiB。ignored 文件不纳入快照，这一限制写入交接上下文。

导出前后、用户启动时、项目暂存前后、投递前分别检查快照。
启动助手再次读取来源并核对 store_identity、历史摘要和最后 turn，防止准备后源继续变化。
同一 canonical worktree 可并行多个 handoff job；每次 Send 生成新 request_id 与独立 job。
同 request_id、同输入仍幂等返回原 job；单 job 靠 revision 与状态机防二次投递。
旧 job 的非终态、Unknown Agent 或 corrupt 记录不阻塞 prepare。
同一 Existing Tab 已有 `Delivering` / `LaunchPending` job 时 route 拒绝发送，提示
`Target tab is receiving a handoff; try again shortly`。检查与写入 Delivering 在同一个
store 锁内完成，避免两个 Send 同时通过；按 existing_target_tab_id 检查所有记录，不按 cwd 过滤。
其他状态和不同目标 Tab 不阻塞。此 guard 不代表目标已完成 turn，也不阻止多 Agent 同时改文件。
快照漂移仍拒绝继续该 job；重新 Send 创建使用新快照的 job，无须先取消旧 job。

## 存储

权威目录：`con_paths::app_data_dir()/handoffs/<UUID>/`。
`bundle.json/context.md/evidence.json` 不可变，`job.json` 原子替换并 fsync。
Unix 目录 0700、文件 0600；拒绝符号链接和非私有/硬链接记录。
跨进程文件锁保护状态写入；释放时显式 unlock，避免继承描述符延迟释放。

仅在用户启动后创建项目内 `.con/handoffs/<UUID>/context.md` 和 `evidence.json`。
存在冲突就拒绝，不覆盖。向 Git 本地 info/exclude 追加当前 UUID 的精确规则，
保留原内容，不改 `.gitignore`。目标只需读取项目文件；短提示不含历史正文或凭据。
Cancelled/Failed 记录从最近状态变更起保留 7 天，后续服务初始化时清理。
活动、投递不确定、包含额外文件或已被用户修改的记录不自动清理。
清理不删除目标服务的会话，也不移除既有 Git 排除规则。

## 状态与恢复

`HandoffJob` 包含 `id/revision/created_at/updated_at/request/state/target/existing_target_tab_id/target_session_id/target_pid/receipt/error`；旧记录的 `existing_target_tab_id` 默认为空。
每次状态修改校验 expected_revision 并递增版本；旧请求报冲突。

| 状态                   | 含义与下一步                                                           |
| ---------------------- | ---------------------------------------------------------------------- |
| Prepared               | 已导出；UI 上 prepare 成功即进入投递，无独立预览确认页           |
| LaunchPending          | 已预留一次启动，等待可见助手；30 秒仍未被助手占用则转 NeedsInteraction |
| StartingTarget         | 已确认执行目标创建/启动；结果未知时不重试                               |
| Delivering             | 创建或现有 Tab 投递意图已持久化，结果可未知；不自动再次发 prompt        |
| AwaitingConfirmation   | 旧版任务的遗留状态（见 2026-09-24 决策）；新流程不再进入               |
| AwaitingManualDelivery | 自动提示未经验证；用户粘贴并确认已发送                     |
| Active                 | 用户发送（或确认已发送）即确认接续，不表示目标已停止。面板核对 pid、进程启动时间和 Agent 身份；目标消失、pid 被复用，或旧记录没有进程身份且当前没有该 Agent 的终端时，自动 Cancelled（只清理卡片） |
| NeedsInteraction       | 用户可放弃未确认投递，已有回执则须检查目标并明确停止；不自动杀进程或取消 job                         |
| Cancelled / Failed     | job 终态；新任务在任何旧 job 状态下均可 prepare                                             |

有 `confirm_sent/confirm_received/confirm_stopped/abandon` 四种明确结果，没有布尔审批。
`confirm_sent` 直接完成任务（用户确认已发送即最终确认）；`confirm_received` 服务于遗留的
AwaitingConfirmation 任务与现有 Tab 在 Delivering 时的崩溃恢复（用户观察目标后显式确认）。
Delivering 仅记录启动/投递意图；写入被观察到（现有 Tab）或带提示的启动被观察到（自动投递）即视为
投递完成并进入 Active，回执仍只代表投递已尝试，不代表目标理解。
Prepared 可直接取消；未确认提交的 Kimi New Tab Delivering 可一步 Cancelled。其他启动后的 Cancel 进入 NeedsInteraction。
ConfirmStopped 还必须取得启动助手锁，目标/助手活跃时拒绝释放。
异常只进入可核对状态；应用重启不重建目标、不重发、不继续排队投递。
已返回的目标 ID 在下一次文件检查前保存，文件变化也不能让这个 ID 丢失。

## 原生启动

UI 绑定原始 tab 的稳定 summary ID 和源 terminal 的 EntityId，拒绝已关闭/替换的来源。
新目标在独立的新 Tab 中启动，源 Tab 与其 surface 保留；显式激活新 Tab。
执行旁边的 `con-cli handoff run <UUID> --revision <N>`，命令路径安全引用。
助手要求真实 TTY，持有每任务启动锁，按选中产品使用独立 argv 启动原生界面。
Cursor create-chat 后指定 ID resume；Codex/Kimi 直接新建原生会话，
启动前无法观测的 native ID 保留为空。
回执确认交接内容已经接续；不把 handoff UUID 或进程 ID 伪装为 native session ID。
子进程继承终端，各产品展示自己的登录、信任和工具权限提示。

逐一探测明确命令和产品 help/version；不使用可能指向其他产品的通用别名（如 `agent`）。
Codex/Cursor 使用已验证的 argv 门控；Kimi 使用读屏门控的 PTY 初始提示。
Kimi 的首次提示通过 Con PTY 自动注入；非交互 `-p` 禁用，手动粘贴仅为失败兜底。
不添加 force、trust 或 yolo；模型覆盖按下节约定单独开放。create-chat 超时 30 秒、输出上限 4 KiB。
没有机器可读 ACK；不将进程启动或屏幕文本推断为“目标已理解”。投递完成的判定见下节：
以用户的显式发送确认为最终确认，以观察到的写入/启动为投递完成的证据。

## 投递确认语义（2026-09-24，用户产品决策）

用户在面板主表单点击 **Send handoff**（自动/现有 Tab 投递）或点击 **“I sent the instruction”**
（手动投递）即为该次交接的**最终确认**。本节取代上文及此前版本中“投递后还需一次
confirm_received 才进入 Active”的要求，是用户明确要求的产品决策，而非技术限制。

- **无独立预览确认页**：REVIEW & SEND 复核页已于 2026-09-24 按用户产品决策移除。
  点击 **Send handoff** 即 prepare 并直接进入路由投递，一次点击完成准备与发送，
  中间不再有第二个确认，也不再有 Discard。交接内容改由任务卡片的
  “View handoff context” / “View evidence” 查看。本条取代任何“在预览页复核后
  再发送”的表述；prepare 与投递之间的完整性校验（来源会话重绑、工作树快照、
  历史摘要核对、模型值校验）全部保留，任一失败即取消 Prepared 任务并报错。
- **Codex/Cursor 现有 Tab 投递**：PTY 写入被观察到后，`record_existing_delivery` 直接把任务置为
  `Active`，回执记为 `delivery_write_observed`（只表示写入已观察，不声称目标理解）。
- **Codex/Cursor 新 Tab argv 自动投递**：初始提示随目标 argv 传入且子进程启动被观察到时，`record_spawn`
  直接把任务置为 `Active`，回执记为 `automatic_delivery_spawn_observed`。
- **Kimi New/Existing Tab**：spawn 后保持 Delivering，只有提交确认才 Active，回执为
  `pty_injection_submit_observed`。`automatic_delivery` 仅指 argv，Kimi 保持 false，序列化不变。
- **失败兜底手动投递**：Kimi NeedsInteraction 可显式 `confirm_received`；旧手动任务 `confirm_sent` 直接把任务置为 `Active`，回执记为 `user_confirmed_sent`，
  不再要求第二次确认。
- **向后兼容**：`AwaitingConfirmation` 状态保留用于反序列化旧任务；旧任务仍可通过
  `confirm_received` 完成确认。崩溃后停留在 `Delivering` 的任务（含现有 Tab 路径）也仍可
  由用户观察目标后显式 `confirm_received` 或释放；系统不自动重发。
- **面板相关性**：只显示与当前 Tab 会话绑定的最新非终态 job：来源 Agent 相同，且
  `source_session_id` 等于活绑定或用户显式选定的会话。按 created_at、updated_at 取最新；
  该卡片不隐藏发送表单。目标进程身份用 `target_pid`、`target_process_start` 和 Agent 核对，
  目标消失时自动 Cancelled，回执 `target_process_absent`；不影响 Send。
  `existing_target_tab_id` 重启后可能重排，不能单独证明进程身份。
  Unknown、NeedsInteraction 或不确定身份保留人工处理；Prepared 到 Delivering 不因缺少 pid 自动取消。

## 源停止确认的移除（2026-09-24，用户产品决策）

「Source work stopped」确认勾选框与 `PrepareRequest.source_stopped` 字段已按用户产品决策移除：
勾选只声明用户已自行停止源 turn，Con 既不校验也不停止任何源进程，该确认没有可执行语义。
本节取代上文及此前版本中要求用户确认“当前 turn 和后台命令已停止”的全部表述。

- **不再确认。** 交接准备与执行不再要求源停止声明；源 Agent TUI 始终保留运行。
- **并发写入风险由用户自担。** 每 job 独立；交接期间源 Agent、目标 Agent、编辑器或其他
  进程仍可能继续写同一工作树，用户应在执行前自行决定何时停止源工作。
- **导出完整性检查保留。** 各来源 reader 仍拒绝导出已知未结束的 turn 或不完整历史
  （如 Kimi 未结束 turn），该拒绝是数据完整性约束，与停止确认无关。
- **向后兼容。** 旧 `job.json` 与旧 CLI 请求中的 `source_stopped` 字段被忽略，仍可读取；
  新代码不再写出该字段。

## 支持范围的收缩（2026-09-25，用户产品决策）

Agent 支持范围按用户产品决策收缩为 **Codex、Cursor、Kimi** 三种：
DimAgent（已移除）、OpenCode、Pi、Gemini、GitHub Copilot、Grok 的来源 reader 与目标启动适配已从
`AgentKind`、`con-agent::handoff` 与 UI/CLI 选项中移除。本节取代此前版本中
“本机 9 个 Agent”的全部表述。

- **范围。** 这 6 种 Agent 既不能作为 handoff 来源（不再导出其会话），也不能作为
  handoff 目标（不再探测、不再启动）。`handoffs.agents`、`handoffs.sources` 与
  `handoff agents/sources/prepare --source-agent/--target-agent` 只接受上述 3 种，
  其他名称报 `Unknown agent`。
- **旧记录。** 已移除的 Agent 名称（包括 `"dimagent"`）经 `#[serde(other)]` 读取为
  `Unknown`，保留 job 和 bundle，不阻塞新 prepare。禁止重新启动、投递和自动取消；
  未启动任务可显式取消，未确认投递可 Abandon，有提交回执须确认停止。
- **无回退。** 不提供别名、不自动映射到其他 Agent；重新支持这类 Agent 需要恢复适配
  并重新通过验收。

## 模型覆盖约定（2026-09-24）

「新建目标 Tab」允许用户在发送前**显式选择一个模型**（精确 ID 输入，部分 Agent 提供候选），
默认不选择，未选择时完全沿用原行为——目标产品使用自己的模型配置，argv 与原状逐字节一致。
本节取代上文及此前版本中“不添加模型覆盖”的禁令；force、trust、yolo 仍然禁止。

- **仅显式选择生效。** 只有用户在本次交接中明确选定模型时，才给本次原生启动增加模型覆盖；
  未选择时不得注入任何模型参数，也不得读取或改写用户全局/项目级模型配置。
- **仅新建 Tab 路径。** 投递到「已有 Tab」的请求 `target_model` 必须为空；已运行进程无法
  更换模型，UI 在该路径不暴露模型选择，start 路径复核为空才允许继续。
- **独立 argv 传递。** 模型值只作为目标 CLI 自己的独立 argv 参数（如 `-m <model>`，
  各产品确切旗标由适配层按安装版本探测）或目标子进程的专用环境变量传递；绝不拼进 shell
  命令行，绝不经过 shell 展开，不写入任何配置文件。
- **值约束。** 持久化前必须经 `con_core::handoff::validate_target_model` 校验：拒绝空值或
  纯空白、控制字符、超过 128 字节的值，以及以 `-` 开头形似 CLI 选项的值。
- **与权限无关。** 登录、信任、工具权限提示保持目标产品原样；模型选择不构成也不改变任何
  权限授予，目标的历史授权不随模型继承。
- **失败语义。** 发送前发现模型值失效、目标 CLI 版本不支持模型旗标或无法确认旗标语义时，
  必须要求**重新准备**（改选模型或取消选择），绝不悄悄改用默认模型启动。启动之后由目标
  TUI/服务判定模型无效（报错、回退或提示）的，走现有 `NeedsInteraction` 核对路径，由用户
  检查目标状态并显式处理，系统不自动重发或自动换模型。
- **UI 措辞。** 界面统一表述为「请求使用模型」，并注明实际生效以目标 TUI/服务为准；
  不得承诺“已切换模型”。目标启动后的真实模型未经产品自身确认前不作断言。
- **兼容与协议核对。** 旧 job（无 `target_model` 字段）反序列化为「不指定」，行为不变。
  模型覆盖把启动助手协议提升为 `LAUNCH_HELPER_PROTOCOL = 2`：Con 派生助手前与助手读取
  job 后都必须核对协议版本，助手不认识模型字段或协议版本不足时**必须拒绝启动**并提示
  升级 con-cli，不得按默认模型静默启动。

## 控制面

`handoffs.open/agents/sources/prepare/list/get/start/respond/cancel` 均注册到现有 JSON-RPC 控制面。
open 和 start 需要 live Con；其他操作也可通过独立 CLI 读写同一存储。
start 接受 `job_id/expected_revision/tab_index/source`，source 复用 SurfaceTarget。
respond 接受 `job_id/expected_revision/outcome`，cancel 接受 `job_id/expected_revision`。
get 返回 job、preview 和短 instruction；prepare 返回 job 和 preview。

## 明确不在本批范围

Windows/Linux、SSH、跨 worktree、Cursor IDE 导入、自动停止原生 Agent、
自动 ACK、模型摘要和持续双向同步均未实现。
本机 3 个产品之外的 Agent 仍需通过 [兼容性研究](../study/agent-handoff-compatibility.md) 的适配门槛。

## 本机多 Agent 类型与向后兼容（2026-09-22）

旧 Codex → Cursor 任务与调用保持可读取；缺少 Agent 字段时按原始默认值迁移。

- `AgentKind`: Codex, Cursor, Kimi；
  序列化名称 codex/cursor/kimi。
- SourceSession 新增 agent（旧记录默认 codex）。HistoryExport 的 agent_version 接受旧 codex_version 别名。
- TargetCapabilities 替代 CursorCapabilities：agent（旧记录默认 cursor）、executable、version、automatic_delivery。
- AgentAvailability：agent、executable: Option<PathBuf>、version: Option<String>、source_supported、target_supported、diagnostic: Option<String>。
- PrepareRequest 新增 source_agent/target_agent，旧请求分别默认 codex/cursor；身份核对必须包含 agent。
- `discover_sessions(agent, cwd)` 与 `export_session(agent, cwd, id)` 返回现有统一来源类型。
- 来源模块契约：每个产品模块 `pub async fn discover(cwd: &Path) -> Result<Vec<SourceSession>>`、
  `pub async fn export(cwd: &Path, id: &str) -> Result<HistoryExport>`。
- `installed_agents() -> Vec<AgentAvailability>`、`probe_target(agent) -> Result<TargetCapabilities>`、
  `create_target(&TargetCapabilities, cwd) -> Result<Option<String>>`、
  `target_args(&TargetCapabilities, cwd, Option<&str>, Option<&str>) -> Result<Vec<OsString>>`。
- protocol::capture(exe, &[OsString], Option<&Path>, limit) -> Result<Vec<u8>> 提供有界子进程只读输出；
  另保留 executable(name)、output(exe, &[&str]) 供能力探测。
- 来源原生 ID 是有界非控制字符串，不能作任意路径；目标 ID 可为非 UUID（如 Kimi 的 `session_*`）。
  target_session_id=None 只表示原生 ID 尚不可观测，UI/回执不得把 handoff UUID 当成原生 ID。
- record_launch 在 StartingTarget 后持久化可选原生 ID，再进入 Delivering；没有 ID 仍禁止再次创建。
  明确用户回执可以确认按 handoff UUID 辨认的接续，不声称验证原生 ID。
- handoffs.agents 列出安装/能力；handoffs.sources 新增 agent；CLI/UI 都支持选择 source_agent 和 target_agent。
- 新增适配器默认手动投递，保留初始提示 argv 实现供最终统一验收后启用；不改变任何权限/模型设置。

## 目标模型选择的向后兼容（2026-09-24）

- `PrepareRequest` 新增 `target_model: Option<String>`，缺字段的旧 job 与旧请求反序列化为
  `None`（不指定），行为与此前版本一致；该字段参与 request_id 幂等比较。
- 启动助手协议版本升为 2（`con_core::handoff::LAUNCH_HELPER_PROTOCOL`）。不识别模型字段的
  旧 con-cli 助手在协议核对不通过时必须拒绝启动，不得静默按默认模型执行，详见
  “模型覆盖约定”一节。

## 2026-09-25 诊断修复：字节保真与显式放弃

- Kimi 注入使用 `ghostty_surface_binding_action` 的 `text:` 原语：逐字节编码
  `\xNN`，Ghostty 解码后以单个 `termio.Message.writeReq` 入队。绕过键盘事件和
  paste API，保留 `ESC[200~instruction ESC[201~ LF`，不改用户键盘、Codex/Cursor argv。
  写入成功仅表示入队；仍须读屏确认提交。pending 检查容忍两端字面 paste marker 残余。
- `Abandon` 是用户显式放弃未确认交接，回执 `user_abandoned`，直接 `Cancelled`。
  可用于无 `pty_injection_submit_observed` 回执的 `Prepared` / `NeedsInteraction`，以及无 existing tab 的
  Kimi `Delivering`。不要求 target stopped，不发送停止信号，也不删除上下文文件。
  有 `pty_injection_submit_observed` 回执时不提供放弃；Active 和待确认态也不提供放弃。
  `ConfirmStopped` 继续要求启动助手锁；Abandon 可在助手仍活跃时取消这一 job。
  迟到的 helper 错误不得复活已取消记录；观察器看到状态或 revision 变化后停止注入。
- 删除历史任务聚合区、展开/隐藏、历史卡片和删除按钮、删除 RPC/CLI 及相关状态。
  当前会话只显示最新相关非终态 job 的卡片，卡片与发送表单并存。
  不再扫描 git_dir 冲突，没有占用者弹窗或 Abandon 后自动 prepare。
  已提交任务改用停止确认路径，不能绕过回执。磁盘记录仍按原保留期管理。
- Codex 分别提示无 writer lock、多 lock/rollout 或跨进程冲突、探测失败、app-server 未连接。
  无进程证据时禁止**无确认**的 recent 自动绑定；探测成功后才可建议同 cwd、
  updatedAt 唯一的会话，必须勾选确认（`confirm it is current`）。并列/无候选不绑定，
  lock/rollout 命中不被 recent 覆盖，探测失败不冒充无锁。
  发送复核若从进程证据降级为 recent，未勾选确认则拒绝发送并要求重开面板。

## 移除 worktree 租约（2026-09-25，用户裁定）

用户认定租约业务“太重”，取代此前保留建议。上述独立 job 与 per-tab guard 为当前并发契约。
corrupt / 缺 job.json 记录继续 warn、跳过列表并保留目录，不 gate prepare；相同 UUID 的记录冲突仍拒绝。
`launch.lock` 是 per-job 启动互斥，`handoff run --revision` 是 job 乐观锁，
`LAUNCH_HELPER_PROTOCOL` 是助手能力版本；三者均保留，与 worktree 互斥无关。
Cancelled/Failed 的 7 天磁盘保留期不变。旧 Agent 进程可能仍存活；Abandon 不停止它。
多 job 可以同时修改工作树，快照仅保护投递前一致性，不承诺投递后串行写文件。

## C 兜底交互（2026-09-25，A2）

- `NeedsInteraction` 的卡片竖排三步：复制结果、目标中的粘贴动作、等待/确认。
  成功复制才显示 `Copied to clipboard`；备份失败或 argv 失败未备份时显示
  `Use Copy again to copy instruction`。`AwaitingManualDelivery` 复用三步，保留
  `I sent the instruction`。卡片文本可换行，480px 面板不截断错误正文。
- `HandoffFallbackGuide` 与 receipt 分离，保存七类原因、剪贴板结果和读屏证据。
  `not_ready / trust / paste_pending / submit_uncertain / launch_timeout /
  argv_bootstrap / target_dead` 分别提供短错误和下一步；trust 明确要求先批准文件夹。
  bundle 可读时 Kimi/Codex/Cursor 均提供 `Copy again`。复制不启动、不重发 PTY。
- Kimi 从 Delivering 落入 NeedsInteraction 后，复用投递前屏幕作为基线继续观察；
  无基线时首屏只建立基线。前台 Tab 每 250ms、后台 Tab 每 1s，单次最多 60s。
  Tab 关闭/终端替换、目标进程或前台组变化、Abandon、Active/终态、外部 revision
  变化及实体销毁立即停止（在下一次轮询检查）；不重新激活 Tab，也不延长窗口。
  无完整进程身份时不猜测已死亡；有 pid/start 且进程消失或被复用才分类 target_dead。
- `PasteDetected` 必须命中 `kimi_instruction_pending` 的完整指令（允许原有 marker
  容差）；部分粘贴、用户编辑、trust 或 `Error: LLM not set` 不构成证据。
  `Submitted` 使用与 B 路径 `pty_injection_submit_observed` 相同的
  `confirm_kimi_submit` 门槛：空编辑器与新指令回显或原生 session 创建证据，拒绝 trust/LLM 错误。
- **本轮只允许 A2**：检测只更新 guide，点亮 `Handoff sent — confirm`；即使检测
  Submitted 也保持 NeedsInteraction，绝不调用 `record_kimi_submit` 或生成 PTY 成功回执。
  用户先在目标按 Enter 提交，再点确认才进入 Active，沿用显式用户确认回执，不声称验证原生 ID。
  检测与确认结构上分离；未来 A1 需要单独裁定，本实现没有自动激活开关。
- 未检测到证据显示灰态 `Waiting for paste…`；检测窗口结束保留最后证据和人工动作，
  不自动重试。Abandon 为 ghost；target_dead/launch_timeout/argv_bootstrap 以 Abandon 为主动作。
  已有 PTY submit 回执仍遵守停止确认语义，不能利用兜底绕过它。
- 面板打开期间后台刷新 durable job，只有 revision/可复制状态变化才重绘；不在 render 读屏。
  乐观锁与 NeedsInteraction 状态双检拒绝迟到观察，尤其不得覆盖 `user_abandoned`。
- Warning 仅补一句 `Paste with ⌘V in the target, then confirm in Handoff`。
  成功仍零模态。con-cli stdout 只保留简短 opening/exit 提示；剪贴板指引由卡片承担，
  原始失败和技术诊断留在 stderr。argv、B 路径注入/回执、raw、协议、per-tab guard 不变。
