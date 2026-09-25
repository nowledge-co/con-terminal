# Handoff 启动的 Kimi Tab 无法再次作为来源

## What happened

通过 Handoff 打开的 Kimi Tab 再次打开 Handoff 时，被 `Running Agent required` 拒绝；
手工启动的 Kimi 正常。已实测的进程树与诊断保留在
`work/reviews/handoff-546ef19c/regression-diagnosis-round9.md`，本次未修改该报告。

## Root cause

`con-cli handoff run` spawn Kimi 后等待子进程退出，前台进程组 leader 仍是 `con-cli`，
Kimi 在同组。来源识别仅检查 leader 的进程名和 argv，且 `con-cli` 不属于允许读屏的
解释器宿主，因此缺少实时证据并提前返回 None；缓存否决未参与这个失败。

## Fix applied

新增 macOS `agent_from_process_group`，通过 `getpgid` 定位当前组，优先检查 leader，
再逐成员匹配 argv 或进程名。与 `group_session_arg` 共用组成员 argv 扫描器；
非 macOS 延续原有不提供进程 argv 识别的行为。

开门判定仍先拒绝 shell，再检查 leader 进程名、进程组证据，最后才允许既有的
node/bun/python/python3 屏幕兜底。缓存仍只否决不代替实时证据，契约不变。
`check_source_binding` 和 Existing Tab 列表复用相同入口，自动受益，无需额外改动。
未改动投递、PTY/A2、协议、成功提示、per-tab guard、租约、Codex peer/recent 路径。

## What we learned

前台进程组 leader 不一定是正在交互的 TUI。身份识别和会话绑定必须采用一致的
实时进程组范围，不能依靠扩大屏幕白名单或缓存品牌绕过缺失的进程证据。

## Validation scope

新增 5 个单测：伪造 KERN_PROCARGS2 字节覆盖 con-cli + kimi/kimi-code/解释器子进程、
普通进程与无关 node 脚本；覆盖进程名兜底、非法 PID、缓存否决、shell 和直接 Kimi
在组扫描之前完成判定。原 binding 测试移入独立文件，保持代码文件少于 500 行。

真实桌面验收待手动执行：Codex Handoff 到 Kimi，新 Tab 再开 Handoff 应进入面板；
shell/vim 仍拒绝，手开 kimi 仍可进入。本次不启动真实 Agent、不触碰用户会话数据。

最终验证（均经 mise exec）：
- `cargo build --workspace`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `just lint`：通过。
- `cargo test --workspace`：950 passed / 0 failed，比基线 945 增加 5 项。

上述四项首次沙箱执行均被 clang ModuleCache 写权限阻断，提权重跑后通过。
首次 Clippy 同时发现新增表驱动测试的 type_complexity，已用类型别名修复。
仍有既有 Cargo 双 binary target 提示、Zig 版本信息及依赖未来兼容提示，无最终检查错误。
