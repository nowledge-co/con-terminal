# Handoff 弹窗打开时崩溃

## 发生了什么

2026-09-23，用户在运行中的 Agent Tab 点击 Handoff 后，Con QA 进程崩溃。崩溃日志显示主线程在 `tokio::process::Command::spawn` 中 panic：`there is no reactor running`。

## 根因

Handoff 弹窗的 `load` 用 GPUI 的 `cx.spawn_in` 运行 `installed_agents()`。该函数会调用 Tokio 子进程探测；GPUI 主线程不是 Tokio runtime 线程。之前在 Shell Tab 测试只走到入口拒绝提示，没有进入弹窗加载，因此未触发这个路径。

## 修复

用 harness 持有的共享 Tokio runtime 调度安装探测和持久任务读取；GPUI task 只等待结果并更新界面。runtime task 失败时在弹窗中显示错误，不再触发主线程 panic。

## 学到什么

`cx.spawn_in` 不提供 Tokio reactor。任何包含 `tokio::process`、`tokio::task::spawn_blocking` 等操作的 future，必须从共享 Tokio runtime 启动；Handoff 验收必须覆盖来源 Agent 运行时真正打开弹窗，而不只验证 Shell 拒绝分支。
