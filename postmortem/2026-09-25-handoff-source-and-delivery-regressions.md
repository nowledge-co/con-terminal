# Agent Handoff 来源与投递回归

## What happened

Cursor worker 启动不带 resume 时无法建议来源；Kimi 包装进程及空 banner 阻断来源选择。
Kimi 无模型候选；新 Tab 大多只开 TUI，手动指令被全屏界面覆盖。

## Root cause

binding 依赖 argv；源识别没有有界屏幕后备，空 banner 被误当作磁盘无历史。
model probe 仅支持 Cursor；自动投递被 Cursor 精确版本锁死；手动提示只有 PTY println。
环境白名单不是这四项的主因，保留 a3441bca，未增加任何环境变量。

## Fix applied

Cursor 复用文件 discover 提供需确认的最近会话建议，未知/并列不绑定。
Kimi 增加明确进程名、解释器屏幕后备和 continue 布尔识别，磁盘会话继续供手选。
模型 probe 只返回经过过滤的 models key，不输出 provider JSON。
Codex 原生 argv prompt 开启；Cursor 日期能力门槛放开；手动投递在启动前复制 instruction。

## What we learned

发现历史、建议当前会话、证明进程身份是不同层次，不能用空 banner 隐藏历史。
原生 TUI 的输入可达性需要单独验收，PTY 写入成功或 spawn 不是目标理解的证据。
Kimi/DimAgent（后续已移除；此处为历史事实） 一次性非交互参数不能直接替换交互式接续。

## Validation scope

回归测试覆盖来源确认、会话选择、模型脱敏、原生 argv、剪贴板写入失败及 Codex 启动状态。
完整 workspace build/clippy/lint/test 结果记录在实施计划；真实桌面多 Agent 接续仍需人工验收。

## Round4 产品裁定：Kimi 自动投递

用户两次报告新 Tab 只有 Kimi 界面、没有后续。根因是旧契约把 Kimi 无交互 argv prompt
等同于必须手动粘贴。用户明确裁定 Send handoff 已是最终确认。

修复：Con 在 New/Existing Tab 共用读屏就绪门控（排除 trust）、bracketed paste + newline
和提交确认；spawn/PTY write 均不足以 Active。注入前静默备份剪贴板，失败进入
NeedsInteraction，卡片提供复制、Cmd-V 及显式确认。Codex/Cursor argv 与 Unknown 租约不变。

教训：原生 argv 能力与交互 PTY 能力应分开；终端接受字节不等于编辑器提交。
自动重试只有明确仍在输入框的证据才安全；未知结果不能盲目重放。
本次隔离 2.1.1 实测 LF 未提交，CR 才触发；因此重试只补 Enter，不重复粘贴。
实录及 919/0 全量测试结果见 `docs/impl/agent-session-handoff-kimi-validation.md`。
桌面及已登录 API turn 验收仍需人工完成。
