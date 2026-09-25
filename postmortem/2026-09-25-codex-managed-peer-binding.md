# Codex managed app-server 分体导致源会话漏检

## What happened

Codex 0.157 TUI 在 Con Tab 中运行时，Handoff 能列出同目录历史，却无法稳定预选当前会话。
round6 已确认 TUI PID 73290 无锁，另一个 pgid 的 daemon PID 75726 持有 rollout/writer lock。

## Root cause

原实现只扫描前台 leader 及其进程组，未覆盖 TCP 对端的 managed app-server。
此外 libproc 返回成功但没有文件证据时直接返回 None，跳过 lsof，造成第二层假阴性。
历史 discover 的 thread/list 与实时文件持有证据是两条不同链路。

## Fix applied

- 匹配完整 loopback ESTABLISHED 连接的反向端点，校验 codex app-server argv 后扫描 peer。
- 保留 leader/pgid 和 rollout 优先；跨来源 ID 冲突、探测失败均拒绝绑定。
- libproc 空结果走有超时/输出上限的 lsof；区分空选择与真实错误。
- 仅成功无证据时建议同 cwd、updatedAt 唯一的最近会话，强制勾选确认；
  prepare/dispatch 复核不能把原 lock 绑定静默降级为未确认 recent。
- 四项全库检查通过，测试从 922 增至 937。真实 daemon 文件检测与 lsof 一致；
  当前无独立 TUI，完整 TUI→peer→面板桌面验收仍未完成。

## What we learned

进程组不等于运行时边界，关联 daemon 必须依靠明确连接关系，不能扫描所有同名进程猜测。
“探测成功但无证据”与“探测失败”必须分开；recent 只能提出建议，不能证明当前线程。
发送时不仅要复核 session ID，还要复核证据是否降级并重新要求用户确认。
Codex 0.157 schema 的 thread/loaded/list 仅表示已加载线程，不证明某个 TUI 的当前视图。
实验隔离必须在子进程入口清空环境，避免 login(1) 恢复真实用户环境。

详见 [验证记录](../docs/impl/agent-session-handoff-codex-validation.md)。
