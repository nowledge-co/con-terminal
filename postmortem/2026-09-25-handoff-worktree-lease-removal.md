# Handoff worktree 租约移除（2026-09-25）

## What happened

用户裁定 worktree 租约业务“太重”，推翻此前保留建议，要求整体移除。
旧非终态 job、Unknown 与 corrupt 记录可能挡住新 Send；面板还会用旧 job 卡片替换发送表单。
执行依据：`work/reviews/handoff-546ef19c/regression-diagnosis-round7.md`，只读保留。
起点 `9e47fcc6`，本地 main 六步提交，不 push、不建分支、不建 PR，不改任何 review 报告。

## Root cause

worktree 独占把互不相关的交接 job 耦合在一起，也把“取消这个 job”变成再次 Send 的先决条件。
真正需要防护的是单 job 的重放，以及同一 Existing Tab 在投递窗口内被两个 job 同时写入。
快照一致性、进程核对、回执与记录保留不依赖 worktree 租约。

## Fix applied

| 步骤 | 文件（路径前缀省略） | 改法与分步测试 |
| --- | --- | --- |
| 1 | core `coordinator.rs/types.rs/store.rs/tests.rs/tests/{abandon,kimi,legacy,lifecycle}.rs`；app `destination/{prepare,lifecycle,tracking,lease}.rs`；core `coordinator/delivery.rs` | 删除 git_dir / corrupt gate、HandoffLeaseConflict、holds_lease；用 is_terminal 同步调用方保证中间提交可编译。核心 handoff 44、UI handoff 47 通过。 |
| 2 | core `coordinator.rs/coordinator/delivery.rs/tests/{legacy,lifecycle}.rs`；app `destination.rs/destination/{tracking,presence}.rs` | lease.rs 改为 presence.rs；classify_target_presence / cancel_absent_target 只做目标消失后的 job 自动 Cancelled；launch_error 保留终态拒绝。44 + 47 通过。 |
| 3 | app `destination.rs/destination/{lifecycle,prepare,view}.rs/destination/view/{cards,fit}.rs`；删除 `tracking.rs` | 删除租约字段、冲突卡片、Abandon 后重发。缓存列表取最新相关非终态 job；卡片与发送表单并存。tracking 并入 destination.rs。44 + 47 通过。 |
| 4 | core `coordinator.rs/coordinator/delivery.rs/types.rs/snapshot.rs/validate.rs`；app `destination.rs/destination/{lifecycle,prepare}.rs/destination/view/cards.rs/route/{mod,reserve}.rs` | Abandon/cancel 只清理这一 job；删除先释放再 prepare 的文案，快照漂移提示新建 job；回执与取消状态转换不改。44 + 47 通过。 |
| 5 | core `coordinator/delivery.rs/tests/existing.rs`；app `route/mod.rs` | Existing Tab 的 Delivering/LaunchPending 原子 guard。核心 46、UI 47 通过。 |
| 6 | core `handoff/mod.rs`；contract、design ADR、plan、kimi-validation 与本 postmortem | 写明并发语义、测试处置、风险和验收。AGENTS.md 无租约条款，无须修改。 |

per-tab 检查由 route 调用的 `begin_existing_delivery` 在已有 store 锁内执行，
与写入 Delivering 同一个事务；route 在写 PTY 前收到简短错误。
这比单独在 route 先 list 再调用服务多一个约束：不会发生检查与写入之间的并发穿透。
无需新锁文件、租约记录或全局运行时 guard；按全部持久化 job 的目标 tab ID 检查，不限来源 cwd。

同 worktree 多 job 可并行，每次 Send 新建 request_id；同 request_id 仍幂等。
单 job 靠 revision 与状态机防二次启动/投递。旧 job 非终态不挡 prepare。
同 Existing Tab 的 Delivering/LaunchPending 挡投递；其他状态与不同 Tab 放行。

## 测试逐项处置

只有 1 项测试真正删除，14 项核心测试改写，新增 2 项。改名不改变测试数量。

| 原测试（con-core::handoff） | 处置 |
| --- | --- |
| abandon::conflict_carries_exact_owner_across_same_worktree_directories | 删除：不再存在冲突所有者类型或 git_dir 互斥。 |
| prepare_is_idempotent_and_worktree_lease_survives_reopen | 改名 prepare_is_idempotent_and_independent_jobs_survive_reopen；独立第二次 prepare 成功，保留幂等、reopen 和 cancel 后 prepare。 |
| creation_and_delivery_are_not_replayed_after_crashes | NeedsInteraction 后新 prepare 成功；原单 job 二次启动/投递拒绝保持。 |
| abandon::abandon_failure_releases_lease_even_with_live_helper | 改名 abandon_failure_cancels_job_even_with_live_helper；删 prepare 阻塞/解锁断言；保留 Abandon、Cancelled、user_abandoned、迟到写入拒绝。 |
| abandon::cancel_new_kimi_delivery_releases_in_one_step | 删除取消后“解锁 prepare”断言；仍验证一步 Cancelled。 |
| abandon::pty_submitted_jobs_never_offer_or_accept_abandon | 删除 prepare 阻塞及释放后可 prepare 的断言；保留 can_abandon=false、respond 拒绝和 ConfirmStopped 成功。 |
| lifecycle::corrupt_job_record_is_skipped_and_blocks_only_its_worktree | 改名 corrupt_job_record_is_retained_without_blocking_prepare；保留目录与跳过列表，prepare 成功。 |
| lifecycle::a_record_that_lost_its_job_file_is_retained_and_blocks_its_worktree | 改名 a_record_that_lost_its_job_file_is_retained_without_blocking_prepare；prepare 成功，保留私有记录与 staging 清扫断言。 |
| lifecycle::a_missing_target_process_releases_the_active_lease | 改名 a_missing_target_process_cancels_the_active_job；仍检查 Cancelled、target_process_absent 和不自动取消 Delivering，删除 holds_lease。 |
| lifecycle::launch_failure_keeps_a_visible_error_and_lease_after_spawn | 改名 launch_failure_keeps_a_visible_error_after_spawn；保留错误、NeedsInteraction 与不可重新 reserve，删除 holds_lease。 |
| lifecycle::manual_delivery_completes_on_the_explicit_sent_confirmation | 仅删 holds_lease 断言；Active、回执、重复确认拒绝保持。 |
| legacy::removed_agents_load_without_losing_their_worktree_lease | 改名 removed_agents_remain_readable_without_blocking_prepare；Unknown 可读、不可 start/deliver、可 cancel；新 prepare 成功。 |
| legacy::removed_dimagent_loads_without_losing_its_worktree_lease | 改名 removed_dimagent_remains_readable_without_blocking_prepare；同上。 |
| legacy::removed_active_agent_requires_explicit_stop_confirmation | 自动清理调用改名；删除 holds_lease，最终显式断言 Cancelled，Unknown 不能自动清理保持。 |
| kimi::kimi_existing_and_fallback_keep_identity_and_lease | 改名 kimi_existing_and_fallback_keep_identity；仅删 holds_lease，投递身份/状态语义保持。 |
| existing::existing_tab_in_flight_guard_is_atomic | 新增：两线程同时投递只有一个成功，败者 revision/state 不变且无 staging；换 Tab 可投递。 |
| existing::existing_tab_guard_only_blocks_pending_delivery_to_that_tab | 新增：枚举全部十种状态；只阻止 Delivering/LaunchPending，跨 cwd 同 Tab 仍阻止，人工转 NeedsInteraction 后允许。 |

UI 原 lease.rs 的 7 项测试全部迁至 presence.rs，仅符号/两项测试名称随语义改名；
route/reserve.rs 的 reserve_errors_classify_into_request_and_helper 仅更新快照错误示例文案。
不新增弱化保护的替代测试。基线 937，实测 937 − 1 + 2 = 938，0 failed / 0 ignored。

## 回归保护与完整验证

全部在 macOS 使用 mise exec 执行，日志目录 `/tmp/con-handoff-validation/`。

| 必须验证 | 真实结果 |
| --- | --- |
| `mise exec -- cargo build --workspace` | exit 0，通过 |
| `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` | exit 0，通过 |
| `mise exec -- just lint` | exit 0，通过 |
| `mise exec -- cargo test --workspace` | exit 0，938 passed / 0 failed / 0 ignored |
| round7 回归保护清单 | 下表逐项匹配完整测试日志，全部通过 |

首次受限测试遇到 Clang 缓存写权限错误；按沙箱规则提升权限重跑后通过。
Cargo 的既有 target/依赖 future-incompatibility 提示仍存在，不是新增 clippy lint 错误。

| 回归保护 | 通过证据（测试名省略模块前缀） |
| --- | --- |
| argv 自动投递 | automatic_delivery_completes_on_the_observed_spawn；codex_without_preallocated_id_completes_only_after_spawn；codex_automatic_prompt_is_one_literal_argument |
| Existing Tab PTY | existing_tab_delivery_is_staged_once_and_completes_on_the_observed_write；existing_tab_unknown_write_can_be_explicitly_confirmed_after_restart |
| Kimi 注入与幂等 | kimi_spawn_waits_for_submit_receipt_and_cannot_replay；stale_failure_cannot_overwrite_cancellation_or_submit；kimi_receipt_rejects_other_agents |
| Abandon | cancel_new_kimi_delivery_releases_in_one_step；pty_submitted_jobs_never_offer_or_accept_abandon（按表改写） |
| 协议与模型 | helper_refuses_a_model_job_it_cannot_understand；target_model_is_validated_and_joins_request_idempotency |
| revision | creation_and_delivery_are_not_replayed_after_crashes（按表改写）；rpc_requires_revision_and_keeps_outcomes |
| 快照与 stage | edits_and_wrong_source_block_delivery_without_touching_changes；stage_preserves_index_untracked_work_and_exclude_rules；existing_tab_delivery_rejects_workspace_changes_before_staging |
| launch.lock | live_helper_cannot_be_released_or_started_twice；launch_timeout_cannot_race_a_live_helper_or_replay_delivery |
| 保留期 | cleanup 三项：a_locked_launch_guard_retains_record_and_cleanup_succeeds、an_extra_or_modified_private_file_keeps_both_directories、a_damaged_record_is_retained_without_blocking_the_rest；retention_never_removes_active_or_user_modified_artifacts |
| Unknown S8 | legacy 三项均通过（新名称见上表）；只读、不可 start/deliver、显式取消；不阻塞 prepare |
| 目标消失 C-4 | presence.rs 原 7 项全部通过 |
| per-tab guard | 新增真实并发和十种状态边界两项全部通过 |

A 类蓝本列出的 14 个源码文件全部处理，其中 tracking.rs 合并删除；冲突测试删除 1 项。
额外修改 presence 命名及 snapshot/validate 文案；Rust 总计 +278 / −403，净减少 125 行。
所有改动后的 Rust 文件小于 500 行（最大 store.rs 496 行）。

B 类行为未改：状态枚举、revision、幂等 prepare、快照算法、单次投递、PTY/argv 回执、
launch.lock、Abandon/ConfirmStopped、7 天清理、LAUNCH_HELPER_PROTOCOL、handoff run --revision。
静态比对 15 个关键 job 函数保持原实现；con-agent、con-cli、con-ghostty、Kimi 注入/读屏、
协议门控、成功提示、cleanup 模块与起点无 diff，覆盖 Codex peer/recent 与 literal argv 红线。
C 类按裁定保留快照与漂移检查；corrupt/Unknown 不阻塞 prepare；presence 自动 Cancelled 仅用于卡片清理。
`work/reviews/` 的 8 个报告逐文件 SHA-256 与改动前一致；未触碰六条 NIT。

## What we learned / 接受的风险

- 用户选择独立 job；取消记录和发送新任务是两个操作，不应相互绑定。
- 同 Tab guard 必须原子检查加状态写入，单独的 list 检查无法防并发穿透。
- guard 只保护投递窗口；Active 后再次 Send 允许，不保证目标 turn 已结束。
- 多目标 Agent 可同时写同一工作树；快照只保护 prepare→deliver，不提供投递后文件写入互斥。
- Abandon 不停止进程。失败/Unknown job 的进程可能仍活着，corrupt 记录保留但无法参与状态识别。
- 面板只显示最新相关非终态 job，旧 job 磁盘保留；没有恢复历史聚合 UI。
- snapshot、revision、单次投递、PTY/argv 回执、launch.lock、协议、7 天终态清理全部独立保留。

## 手工验收（本轮未执行桌面交互）

1. 同 repo 留一个 NeedsInteraction job，重新打开对应源 Tab 的 Handoff。卡片与表单并存；
   选另一个目标或新 Tab 直接 Send，应新建并投递 job，不出现冲突/Abandon and continue。
2. 同一 Existing Tab 第一条 job 停在 Delivering（或已有 LaunchPending+tab ID），再次 Send：
   显示 Target tab is receiving a handoff，第二条不写 PTY；改选另一 Tab 可发送。
3. CLI prepare 后修改 tracked/untracked 文件，再对该 job start/投递：快照拒绝；
   使用新 request_id prepare 获得新快照后可继续，无须先取消旧 job。
4. Kimi 已有 pty_injection_submit_observed 时不得 Abandon，仍走 ConfirmStopped；
   Codex/Cursor argv 与 Kimi raw/CR 注入成功不弹额外模态。

## 范围与偏离

- 为中间提交可编译，Step 1 同步改掉 UI 的租约类型引用及 is_terminal 调用方。
- per-tab 原子检查落在 route 调用的核心事务中，而非 route 独立扫描；这是必要的并发保护。
- 同步更新额外 design ADR，避免它继续声称保留租约。
- 未处理六条 NIT；未修改 work/reviews 或历史 postmortem。桌面真实 Agent 操作未重新验收。
