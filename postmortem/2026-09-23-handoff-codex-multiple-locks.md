# Handoff 识别失败：Codex resume 过渡期持有多把 writer lock

## 发生了什么

2026-09-23 在 QA 窗口中对运行中的 Codex TUI 点击 Handoff，弹窗显示
"Couldn't identify the current session in this Tab"。日志显示绑定失败原因：
`Codex process holds multiple thread locks`。

## 根因

绑定逻辑以 `thread-writer-locks/<id>.lock` 文件描述符作为当前 thread 的唯一
证据，并要求组内锁 ID 唯一。但 Codex TUI 在 resume 切换的过渡期会同时持有
旧 thread 和新 thread 两把锁（本例中进程 66741 先为新空会话持锁，resume
`01a0ccd4…` 后短暂双持），唯一性检查直接报错，绑定失败。

实测同一进程此时只持有**一个** rollout 文件描述符：
`sessions/2026/09/23/rollout-…-01a0ccd4-….jsonl`，即 TUI 正在写入的 thread。

## 修复

`crates/con-agent/src/handoff/codex.rs` 引入 `ThreadEvidence`：rollout fd 为
主证据（始终唯一指向正在写入的 thread），锁为回退；两者分别做唯一性校验。
lsof 回退路径同样按两种证据解析。新增测试覆盖"多锁 + 唯一 rollout"的
resume 过渡期场景。

## 教训

- "进程持有的资源"作为身份证据时，要区分**长期持有**（锁，过渡期会叠加）
  和**排他持有**（正在写入的 rollout fd）；后者才是"当前"的可靠信号。
- 唯一性校验失败时应先怀疑证据粒度假设，而不是扩大猜测范围。
