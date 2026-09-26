# Handoff 第二轮修复交付记录（2026-09-25）

范围：B → C → A → D，本地 main；不 push、不建分支、不创建 PR。
起点 d075d4b8。work/reviews 下报告未修改，未跟踪 work/ 原样保留。
6 条 NIT 不在本次范围。

## B：环境防御与失败可观测

文件：
- con-agent：handoff/environment.rs。
- con-cli：handoff.rs、handoff/diagnostics.rs。
- con-app：agent_handoff/destination/lease.rs；启动提示由 A 的 launch_notice.rs 接入。
- con-core：handoff/tests/lifecycle.rs 的失败持久化回归。
- postmortem/2026-09-25-handoff-bootstrap-defense.md。

过滤所有空环境值；Codex 防御性保留 CODE_ASSIST_ENDPOINT。
目标失败记录目标类型、版本、去重排序的 launch env 键名集合，绝不输出环境值。
原生 stdin/stdout/stderr 仍继承 TTY，不捕获或改写 TUI 协议。
非零退出（包括 bootstrap 退出和信号终止）不再返回成功：
提示检查 bootstrap/login/network/proxy/assist，进入 NeedsInteraction，不自动重试。
失败记录不因目标退出而在面板打开时自动释放。
用户错误与诊断分行，避免键名 API_KEY 的整行脱敏吞掉用户错误；完整键集合在助手 stderr。

新增测试：键名诊断无值泄漏、失败退出不成功、失败任务保留可见错误/租约、
面板保留失败任务。既有环境隔离测试新增空 HTTP_PROXY 断言及 endpoint 白名单检查。

**不能声称已修好原错误**：诊断中 exact account/read routing discovery timeout 本机未复现，
argv prompt 已实测无害。当前只补防御、可观测和失败语义；仍需用户环境复验。

## C：Codex 模型候选

文件：con-agent/src/handoff/model.rs、model/codex_cache.rs、
model/codex_cache.fixture.json。

只打开 CODEX_HOME（非空优先）或 HOME/.codex 下的 models_cache.json；
反序列化结构仅 fetched_at、models[].slug/visibility。绝不打开 auth.json/config.toml，
未知字段由 serde 忽略，不遍历认证字段。复用 push_candidate 去重/校验，最多 64 个候选。
文件读取上限 4 MiB，内存缓存 600 秒且路径参与缓存键。
过期文件候选继续尽力展示，debug 日志标记 degraded；不触发网络刷新。
缺失/坏 JSON 返回空候选，保留手输。

新增 4 项测试：fixture 投影与假认证字段不泄漏、路径优先级、缺失/坏 JSON 降级、
600 秒内存缓存过期。既有 side-effect-free probe 测试明确 Codex 不走子进程。

## A：协议门控与手动投递提示

文件：con-app 的 agent_handoff.rs、launch_notice.rs、
route/{mod,protocol,reserve}.rs；con-cli/src/handoff.rs。

所有 dispatch（新 Tab、已有 Tab）前都检查相邻 con-cli 的协议 ≥2，与 target_model 无关。
缺失、低版本、坏 JSON、非零退出、超时及超量输出均拒绝；提示同版本重建，
保持 sibling 路径、不从 PATH 寻找替代 helper。
2026-09-25 用户裁定更新：Kimi spawn 后保持 Delivering，Con 就绪检测、PTY 注入、
提交确认后才 Active。卡片非模态显示 Instruction sent to Kimi；NeedsInteraction 才提供 Cmd-V。
旧 AwaitingManualDelivery 保留为手动兜底，成功路径不弹窗。
观察窗口 30 秒；后续失败仍有任务卡片。Codex argv 自动投递未改。

新增 2 项测试：无模型也拒绝缺失/旧 helper；手动提示等确认、只显示一次并可显示失败。
helper 原有测试同步验证无模型协议 1 也拒绝。定向 UI 测试 32/32 通过。

## D：移除第四种适配

当前仅 Codex、Cursor、Kimi；DimAgent 已移除。
按诊断 8 步顺序移除 enum、reader、agent 分支、core 校验、UI、CLI 测试，再更新文档与兼容测试。

文件：
- con-agent/handoff：kind、sources/mod（删除 sources/dimagent.rs，已移除适配）、
  binding、environment、inventory、launch、target；删除 cli_support 中仅旧 reader 使用的两个函数。
- con-core/handoff：types、tests、tests/legacy、tests/lifecycle。
- con-app/workspace：agent_handoff、destination、prepare、select、route/mod；
  tab_presentation 与 tab_presentation/{agents,tests}。
- con-cli：handoff/manual_delivery（失败 fixture 改用 Kimi）。
- docs：proposal、contract、design ADR、plan、compatibility；两篇历史 postmortem 仅加已移除标记。

S8：持久化旧名称由 serde(other) 读取为 Unknown，保留 job/bundle/租约；
拒绝 reserve_start 和 existing delivery。未启动可显式取消；已启动须明确确认停止。
新增旧名称 lease fixture、三种可选 Agent 断言；Active 兼容测试改用实际旧字符串反序列化。
删除不再可达的模型覆盖拒绝测试和旧 SQLite/WAL reader 测试。

为满足本次修改代码文件 <500 行，既有超长 tab_presentation 与 core tests 拆出模块；
除删除旧品牌映射/旧测试外，移动部分保持原逻辑，不扩展到 NIT 清理。

## 已移除名称的逐条残留清单

以下是生产代码清理后的逐条审计。代码仅有 legacy 测试 3 行；其余为明确已移除的历史/兼容说明。
本文也是已移除范围的交付记录，文件名与兼容说明中的名称不代表恢复支持。

| 位置 | 保留理由 |
| --- | --- |
| [crates/con-core/src/handoff/tests/legacy.rs:9](../../crates/con-core/src/handoff/tests/legacy.rs#L9) | 已移除适配的 fixture 测试名 |
| [crates/con-core/src/handoff/tests/legacy.rs:10](../../crates/con-core/src/handoff/tests/legacy.rs#L10) | 已移除名称作为 source/target：读取 Unknown，保留租约，取消后可重新准备 |
| [crates/con-core/src/handoff/tests/legacy.rs:60](../../crates/con-core/src/handoff/tests/legacy.rs#L60) | 已移除目标的 Active 记录：禁止自动释放，必须明确确认停止 |
| [docs/design/agent-session-handoff-contract.md:226](../../docs/design/agent-session-handoff-contract.md#L226) | 当前契约明确记载已移除范围或旧记录兼容 |
| [docs/design/agent-session-handoff-contract.md:234](../../docs/design/agent-session-handoff-contract.md#L234) | 当前契约明确记载已移除范围或旧记录兼容 |
| [docs/design/agent-session-handoff.md:201](../../docs/design/agent-session-handoff.md#L201) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/design/agent-session-handoff.md:216](../../docs/design/agent-session-handoff.md#L216) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/design/agent-session-handoff.md:227](../../docs/design/agent-session-handoff.md#L227) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/design/agent-session-handoff.md:236](../../docs/design/agent-session-handoff.md#L236) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/design/agent-session-handoff.md:260](../../docs/design/agent-session-handoff.md#L260) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:6](../../docs/impl/agent-session-handoff-plan.md#L6) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:180](../../docs/impl/agent-session-handoff-plan.md#L180) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:201](../../docs/impl/agent-session-handoff-plan.md#L201) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:208](../../docs/impl/agent-session-handoff-plan.md#L208) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:219](../../docs/impl/agent-session-handoff-plan.md#L219) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:223](../../docs/impl/agent-session-handoff-plan.md#L223) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:241](../../docs/impl/agent-session-handoff-plan.md#L241) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:246](../../docs/impl/agent-session-handoff-plan.md#L246) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:256](../../docs/impl/agent-session-handoff-plan.md#L256) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:282](../../docs/impl/agent-session-handoff-plan.md#L282) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:339](../../docs/impl/agent-session-handoff-plan.md#L339) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:354](../../docs/impl/agent-session-handoff-plan.md#L354) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:355](../../docs/impl/agent-session-handoff-plan.md#L355) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:381](../../docs/impl/agent-session-handoff-plan.md#L381) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/impl/agent-session-handoff-plan.md:385](../../docs/impl/agent-session-handoff-plan.md#L385) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/study/agent-handoff-compatibility.md:6](../../docs/study/agent-handoff-compatibility.md#L6) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [docs/study/agent-handoff-compatibility.md:181](../../docs/study/agent-handoff-compatibility.md#L181) | 历史范围、适配依据、路径或验收事实；该处明确标注已移除 |
| [postmortem/2026-09-25-handoff-review-followups.md:11](../../postmortem/2026-09-25-handoff-review-followups.md#L11) | 保留原复盘事实，逐处标明后续已移除 |
| [postmortem/2026-09-25-handoff-source-and-delivery-regressions.md:25](../../postmortem/2026-09-25-handoff-source-and-delivery-regressions.md#L25) | 保留原复盘事实，逐处标明后续已移除 |

## 验证

所有命令均经 mise exec -- 执行，完整输出保存在 /tmp/con-handoff-*.log：

| 命令 | 结果 |
| --- | --- |
| cargo build --workspace | 成功 |
| cargo clippy --workspace --all-targets -- -D warnings | 成功 |
| just lint | 成功 |
| cargo test --workspace | 894 passed / 0 failed |

基线 884：新增 12 项（B 4、C 4、A 2、D 2），删除旧适配专属测试 2，净增 10。
现存 manifest 双 binary 和第三方 future-incompatibility 提示仍存在，不是 Clippy 失败。
过程中的沙箱 Clang 缓存拒绝已通过授权重跑；测试宏导入和删除分支后的 Clippy 问题均已修正。
git diff --check 与 cargo fmt --all -- --check 通过。

## 用户手工验收

1. 同一 shell 重建 Con 与 con-cli，启动相邻的 target/debug/con；
   检查 target/debug/con-cli handoff protocol 为 2。
2. Codex：选中一个 cache slug 发起交接，检查 argv 含独立 --model 及 -- instruction，
   验证目标实际读取 context.md/evidence.json。模型候选仅为请求，不保证服务端接受。
3. Kimi：目标就绪后自动出现 instruction 并开始 turn，无需 Cmd-V；trust 态不注入，超时卡片显示兜底。
4. 在独立测试构建目录放协议 1/缺失 helper，未选模型同样应明确拒绝；不要覆盖正式安装。
5. 旧记录：含已移除名称的任务显示 Unsupported Agent，租约仍阻挡新 prepare；
   显式取消/确认停止后可重新准备，不能自动丢失。

B 用户环境复验（不要发送任何环境值或认证文件）：
- 在同一终端、同一 cwd 下记录 codex --version，对照失败诊断的 target/version。
- 从该 shell 启动刚构建的 Con，再与直接运行
  codex --cd <cwd> -- "<同一短 instruction>" 对比，保持相同登录和网络条件。
- 在本地检查 shell 中代理/TLS/CODEX_HOME/CODE_ASSIST_ENDPOINT 哪些是未设置、空值或非空。
  对照失败日志 launch_env_keys；应没有空代理，非空 CODE_ASSIST_ENDPOINT 应在键集合。
  shell 任意环境名可用 Python 的 sorted(os.environ) 查看，切勿输出 dict(os.environ)。
- 在子进程中分别用 env -u HTTP_PROXY -u HTTPS_PROXY 与
  env HTTP_PROXY= HTTPS_PROXY= 对比直接启动；也比较保留 endpoint 与
  env -u CODE_ASSIST_ENDPOINT。仅在当前网络允许时做这些诊断，不修改持久配置。
- 若仍出现 exact 错误，记录时间、版本、键名集合及原始终端错误，检查 Codex 自身日志、
  assist 服务和代理可达性；env 值仅在本机比较。handoff 不自动重试；
  检查并明确停止目标/后台工作、释放旧任务后才重新准备。

## 未完成与边界

本轮未执行真实桌面点击和外部 Agent 登录后的端到端接续；
B 的 exact 用户环境错误未复现，不能报告“已修复”。
新增非零退出映射覆盖 bootstrap 失败，但不把所有非零退出都诊断为 bootstrap 根因。
协议 ≥2 是能力门槛，不证明二进制同 commit；仍须用户同版本重建。
三 Agent 产品范围及保守旧租约规则已保存为 Nowledge decision；
检索用 nmem thread search（con-terminal handoff，4 条）和 memory search（当前范围，5 条），
本次实现以用户诊断/契约为准。未验证浏览器同身份/空间，因此未展开记忆图谱。
