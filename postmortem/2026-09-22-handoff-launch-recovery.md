# Agent Handoff 启动与恢复边界

## What happened

交接实现的回归测试发现，助手锁释放后，在全套并行测试中偶尔仍无法立即重新获取；
同一测试单独运行通过。检查还发现，create-chat 返回 ID 后，如果工作区同时变化，
状态更新会因快照检查失败而没有保存刚创建的目标 ID。

## Root cause

锁最初只依赖 File 的 close 释放。Rust 在 macOS 上使用 flock；共享的 open file
描述符在 fork/exec 的间隙仍可保留锁，因此仅关闭一个描述符不等价于明确释放。
目标 ID 则与下一次快照检查写在同一个原子更新中，验证失败会连同有用的恢复事实一起丢弃。

## Fix applied

为存储锁和启动锁增加 RAII guard，在 drop 时显式 unlock。
新增复制文件描述符的确定性测试，验证原 guard 释放后可重新获取锁。
目标创建成功后先持久化 ID，再在发出提示前独立验证工作区。
文件漂移仍阻止投递，但恢复记录保留已经发生的创建结果；不自动再次创建。

另给 Con surface 启动增加 30 秒超时：只有任务仍为原 LaunchPending 版本、且没有
存活助手持锁时才能转 NeedsInteraction。晚到的 helper 会因版本过期拒绝运行。

## What we learned

幂等不只约束成功路径，也要保存失败前已经发生的外部事实。
租约释放必须与助手生命周期一致，不能用 UI 取消、PID 显示或窗口关闭推断后台工作停止。

用户随后确认验收期间 Mac 处于锁屏／远程无可见桌面状态。
原生窗口布局须在解锁且有可见桌面时复测；尚不能据此排除代码问题或宣称界面验收通过。
具体证据和待验收项见 [实施记录](../docs/impl/agent-session-handoff-plan.md)。


## 多 Agent 扩展时的提前回执问题

统一静态审查发现，原状态机允许 Delivering 直接确认接续。扩展为手动目标后，
该状态可能只是记录了启动意图，还未 spawn、更未粘贴提示；UI/RPC 的提前确认
会把任务改成 Active，随后助手 record_spawn 又因 revision 变化无法记录 PID。

根因是将“投递结果不确定”和“可以确认已接续”放在同一个状态入口。
现仅允许 AwaitingConfirmation 接收回执，UI 同样移除 Delivering 的确认按钮；
手动目标必须先成功记录启动，再明确 ConfirmSent，之后才能 ConfirmReceived。
新增回归断言覆盖 native ID 未知时的启动前两种回执拒绝。创建/投递崩溃后仍需检查和
停止既有目标，不通过提前回执绕过恢复，也不自动重放。

来源格式审查同时发现 OpenCode export 可能保留已撤销消息，Grok updates 可包含 rewind marker。
两者的未解决边界现在保守拒绝导出，并用 fixture 验证，避免弃用目标成为交接的最新请求。
