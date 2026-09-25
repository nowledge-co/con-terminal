# Handoff 字节注入与租约恢复

## What happened

Kimi 输入出现 paste marker 残余，未提交后进入失败态并占用 worktree 租约。
历史聚合区的释放入口难以找到，Codex 无进程锁时的手选提示也不够明确。

## Root cause

通用终端写入将 ESC 拆成键事件，破坏 bracketed paste 字节序列。
pending 精确匹配又被残余 marker 阻断。取消未完成交接只进入 NeedsInteraction，
释放操作还要求用户声明目标停止。租约恢复与历史 UI 耦合。

## Fix applied

Kimi 用 Ghostty text binding 解码字节并单次入队；键盘与 argv 保持原样。
无投递回执的交接可显式 Abandon，记录放弃回执，迟到错误不能复活租约。
删除历史聚合区与删除接口，把占用者及恢复动作放入 prepare 冲突提示。
保留当前任务卡片和已确认投递的 ConfirmStopped。Codex 分开无锁、多锁、检查失败文案。

## What we learned

PTY 字节接受不等于按键编码，更不等于 TUI 提交。协议序列需要单独的字节保真入口。
删除任务列表之前，必须迁移它承担的资源释放职责，并覆盖迟到回调和跨目录租约冲突。
