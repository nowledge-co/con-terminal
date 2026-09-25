# Kimi PTY 自动投递 — 2026-09-25

用户裁定：Send handoff 即最终确认；Kimi 应自动接收，不要求 Cmd-V。
方案基于 round4 诊断，Codex/Cursor argv 门控、Unknown 读兼容和协议检查保持不变。

## 实现与证据

- `con-agent::handoff::kimi_delivery_payload` 编码为 `ESC[200~instruction ESC[201~ LF`。
- New/Existing Tab 共用 Con 侧观察器：静默 pbcopy → 激活目标 → 读屏就绪 → 注入 → 提交确认 → 持久回执。
- 每 250ms 读取屏幕，最多 120 次；计时只是轮询节奏，不构成就绪证据。
  Welcome/Session + 空输入框才就绪，Trust this folder / Don't trust 优先阻止注入。
- Kimi 2.1.1 输入框带 `│` 边框，支持换行折叠。已有文字时不覆盖。
- 空输入框 + 新指令回显，或空输入框 + 新 session ID 才算提交；旧屏幕、PTY 写入成功都不算。
  `Error: LLM not set` 明确拒绝成功。
- spawn 后仍 Delivering；提交确认写入 `pty_injection_submit_observed` 才 Active。
  `automatic_delivery` 保留 argv 专用语义及旧 JSON 形状；没有添加新枚举状态。
- 不确定即 NeedsInteraction，保留 job 供核对。卡片提供复制、Cmd-V、Confirm visible continuation 和 Abandon handoff；有提交回执则须确认停止。
  成功卡片为 Instruction sent to Kimi，无模态窗；没有额外发送确认。
- 已有终端在注入时重核 Tab、terminal、进程启动时间及前台进程组；观察期间取消会阻止后续写入。
  状态写入带 revision，过时失败不能覆盖新决策。

## 隔离 PTY 冒烟及对诊断的修订

使用本机 Kimi 2.1.1，临时 HOME/XDG_CONFIG_HOME/XDG_DATA_HOME/cwd，空凭据环境。
没有访问用户 `~/.kimi-code` 的配置或会话；仅执行已安装的 Kimi 二进制。
实验结束 kill 该实验进程组、wait 回收、删除临时 HOME。

1. 新 cwd 出现 Trust this folder，未向 trust 对话框注入 instruction。
2. 实验脚本确认临时目录 trust 后，等待 Welcome 和完整的空输入框可见。
3. 指定的 bracketed paste + LF 进入输入框，但本次实测没有提交。
4. 2 秒后对仍完整停留在输入框的相同指令发送 CR（Enter），输入清空，出现
   `Error: LLM not set, send "/login" to login`。未登录环境没有完成 API turn，不能称为接续成功。
5. 脱敏屏幕保存在 `crates/con-app/src/workspace/agent_handoff/fixtures/kimi-*.txt`，测试验证失败屏幕不会误报成功。

**唯一行为偏离**：诊断建议确认失败后重发整段 bracketed payload；实际改为
至少 2 秒后、仅当屏幕证明完整同一指令仍在编辑器中时，重试一次 CR。
重复粘贴会追加或重复用户 turn，因此不对未知结果盲目重发。用户编辑过的输入不触碰。
首次载荷保持报告规定的字节序列。此恢复方式仍需真实登录桌面验收。

## 手工桌面验收（未执行）

1. 同构建启动 Con 与相邻 con-cli，打开 experimental handoff。
2. **New Tab / 已 trust cwd**：Codex 来源选择 Kimi，Send 一次；无需 Cmd-V，
   检查完整 instruction 出现在 Kimi 会话、开始 turn，并读取 context.md。
   Handoff 为 Active、回执为 pty_injection_submit_observed；成功无弹窗。
3. **Existing Tab**：同 cwd 打开空闲 Kimi，从 Cursor Send 到该 Tab；验证与上一项相同。
   预先在 Kimi 输入草稿时，Con 不覆盖也不自动提交草稿。
4. **新 cwd trust**：在未 trust 目录 New Tab。保持 trust 对话框时应无指令/escape 字节进入；
   在观察窗口内手动信任后应继续自动投递。保持不选至超时，应 NeedsInteraction，非 Active。
5. **失败兜底**：目标就绪前退出 Kimi、或阻断提交；确认卡片提供复制/Cmd-V，job 仍可核对且不阻塞新 prepare。
   实际粘贴、发送并看到接续后再点击 Confirm visible continuation；停止目标后可取消该 job。
6. **回归**：Codex→Codex、Codex→Cursor argv 自动投递；Existing Codex/Cursor 行为不变。

## 验证记录

| 命令 | 结果 |
|---|---|
| `mise exec -- cargo build --workspace` | 通过，exit 0 |
| `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` | 通过，exit 0 |
| `mise exec -- just lint` | 通过，exit 0 |
| `mise exec -- cargo test --workspace` | 919 passed / 0 failed；基线 905，新增 14 |
| 隔离真实 PTY | trust、完整输入框、粘贴、Enter 恢复已观察；无模型，未完成 API turn |

新增测试：payload 1；屏幕 fixture/确认 7；共用投递状态机 2；core 回执/失败/竞态 4。
原有测试全部保留。Codex literal-argv 测试、target automatic_delivery 门控函数和
con-cli `automatic_delivery.then_some(prompt.as_str())` 与 d4064a4d 逐字核对一致。
改动代码文件均少于 500 行；work/reviews 不修改；实验子进程已回收。

桌面、真实登录 API turn 与各 Kimi 新版本的屏幕布局均不由单元测试或隔离未登录
PTY 冒烟代替。编译有原有 Cargo 多 binary target 和第三方 future-incompat 提示，无检查失败。

## 21 处契约对齐

| # | 落点 | 完成内容 |
|---|---|---|
| 1 | contract 16–19 | Kimi PTY 主路径、剪贴板兜底 |
| 2 | contract 174 | 交互首次提示通过 PTY，排除 -p |
| 3 | contract 191–196 | 区分 argv/write/submit 回执 |
| 4 | contract 195–196 | Kimi 确认提交后 Active；手动仅恢复 |
| 5 | plan 49–52 | 用户流程自动投递、Cmd-V 仅兜底 |
| 6 | plan 196 | Kimi 2.1.1 PTY 目标表、待桌面验收 |
| 7 | plan 380–381 | Con 统一静默备份，非 helper 手动主路径 |
| 8 | plan 400 | 成功 Instruction sent to Kimi，失败 Cmd-V |
| 9 | round2-validation 53 | Delivering → 提交确认 → Active |
| 10 | round2-validation 142 | 自动 instruction / turn 的验收步骤 |
| 11 | launch.rs 53 | PTY 首次提示注释 |
| 12 | launch.rs 115–118 | 仍拒绝 Kimi argv prompt，错误说明 PTY |
| 13 | target.rs / agent types.rs | argv-only 语义注释，门控与序列化不变 |
| 14 | inventory.rs | Kimi 自动 PTY 能力诊断 |
| 15 | con-cli/handoff.rs | Kimi 剪贴板备份转由 Con 统一处理 |
| 16 | launch_notice.rs | Cmd-V 降为失败/恢复语义，成功不弹窗 |
| 17 | cards.rs | 注入中/成功/失败卡片、复制及显式恢复 |
| 18 | core/types.rs | 旧手动态标签说明自动投递不可用 |
| 19 | coordinator/delivery.rs | spawn 不 Active，新 submit 回执 |
| 20 | route/mod.rs | Kimi 共用 bracketed + submit 分支注释 |
| 21 | postmortem | 追加用户裁定、根因、修复与教训 |

## 决策回查

使用 nmem CLI 检索 `Kimi handoff PTY injection automatic delivery`，请求默认模式，
返回模式 fast_bm25_vector，scope default，无额外 filters，limit 5。
首位相关结果：`ba33d9d5-bcac-4e68-a76c-53f1ca1e11c5`，
《con Handoff 四项回归修复与桌面验收边界》，score 0.6981481752926372。
保留其中 -p 非交互及 write ≠ submit 结论，手动主路径由本次用户裁定取代。
未生成记忆图：当前无法确认独立图形入口执行同一 default 空间访问约束。

## 第五轮修复的验证边界

- macOS raw write 复用现有 C API 的 `text:` binding；全字节 `\xNN` 编码，经 Ghostty
  `config/string.zig` 解码并单次 `Message.writeReq`。不经过 `char_to_key_event`。
- 自动测试使用 mock binding decoder + 字节 sink，断言 ESC 和全部 0–255 字节保真；
  pending fixture 覆盖单端/双端字面 marker 以及拒绝额外草稿。
- 新 prepare 不依赖 Abandon；已有提交回执仍须 ConfirmStopped。Abandon 只取消该 job。
- 桌面需验收：codex→kimi 无字面 `[200~`，开始 turn 并读取 context；失败卡片
  一次 Abandon 取消。旧失败 job 存在时直接 Send 应成功，无占用者弹窗。
  同 Existing Tab 的 Delivering/LaunchPending 连发应被 guard 拒绝；有提交回执不得 Abandon。
  改动快照后继续该 job 仍拒绝，重新 Send 使用新快照。

### 本轮 native 实测

经真实 macOS Ghostty surface 的 `write_raw_to_pty`，PTY 子进程收到：
`1b5b3230307e5265616420636f6e746578741b5b3230317e0a`，与发送 payload 完全一致。
第二次探针在 PTY 子进程入口清空环境并重设 HOME、KIMI_SHARE_DIR、XDG 路径。
Kimi 2.1.1 屏幕无字面 marker；补 CR 后输入清空并显示 `Error: LLM not set`。
证明字节和提交路径可达，不声称登录后的 context.md 接续已验收。

实验偏离：首次仅设置父进程临时 HOME，macOS Ghostty 的 login(1) 恢复了登录环境，
误用用户 Kimi 配置并产生一次真实探针响应。已终止该探针，将新增探针会话/信任/
历史文件移出用户目录，并清除对应索引条目；共享凭据自动刷新、日志和缓存没有
事前快照，未回滚。该次不计入隔离验收。后续 native 测试必须在 PTY 内再次清空环境。

## C 路径桌面验收（A2，2026-09-25）

以下是人工验收用例，新增单元测试不代替真实桌面操作。使用本次构建启动 Con，
在原来源 Tab 打开 Handoff 查看卡片；目标 Tab 保持 Kimi 2.1.1。不要用真实任务重复注入。

| 场景 | 操作 | 预期 |
|---|---|---|
| Trust 超时 | 向尚未批准目录的 Kimi 发起 handoff，保持 trust 屏至 30s | NeedsInteraction；红字 `Approve folder trust in Kimi first`；三步竖排，第二步 `Tap Trust this folder, then ⌘V`；确认灰态，Abandon ghost |
| 手动粘贴 | C 触发后 60s 内批准 trust，⌘V 粘贴完整指令 | `Copied to clipboard`、`Paste in Kimi — ⌘V`、点亮 `Handoff sent — confirm`；此时仍 NeedsInteraction，无 PTY submit receipt |
| 人工确认 | 在 Kimi 按 Enter 提交，回来源 Handoff 点击确认 | Active + 用户确认回执；不弹成功模态，不自动发送第二份指令 |
| 假证据 | 粘贴部分指令、编辑成其他文字、保留 trust、或出现 LLM not set | 不点亮；无自动提交/重发 |
| 失焦/截止 | 切换其他 Tab，等待超过 60s 后再粘贴 | 后台降为 1s；60s 后停止观察，不延长、不重发；保留 Copy again/Abandon |
| 取消/关闭 | 观察中 Abandon，或关闭/替换目标 Tab | 最迟下一次轮询停止；迟到观察不能复活 Cancelled 或覆盖 user_abandoned；Abandon 不杀目标 |
| 进程退出 | C 期间让有 pid/start 的目标退出 | `Target process ended`，主动作 Abandon；无 pid 的不确定启动不误报已死 |
| 备份失败 | 隔离环境模拟 pbcopy 失败，再点 Copy again | 第一步先显示复制补救提示；点击后显示 Copied to clipboard，不假报自动复制成功 |
| Codex/Cursor | 模拟 argv/bootstrap 失败并保留可读 bundle | 三步指引、Copy again、Abandon；不使用 Kimi 读屏，不自动重发 |
| 超时 | helper 未启动至 launch pending 30s 截止 | `Target did not start in time`，第二步要求打开目标或重新发送，主动作 Abandon |
| 窄面板 | 默认 480px 宽度，逐项查看七类卡片及较长旧 error | error 和三步文案可换行，无水平裁切；按钮不挤出卡片，无新增 border/shadow |
| CLI/模态 | 观察 helper scrollback 及 Warning | spawn 前无 clipboard stdout；失败 stderr 保留；Warning 含简短 ⌘V 指引；成功零弹窗 |

自动化覆盖：fallback 读屏 fixture（Idle/PasteDetected/Submitted 与反例）、七类三步文案
与按钮优先级、trust 分类、迟到观察/Abandon/Active 保护、Warning 文案、helper stdout。
