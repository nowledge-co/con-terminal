# Handoff bootstrap 防御与可观测

## What happened
用户报告 Codex account/read / workspace routing discovery timeout。本机诊断未复现；
带 argv prompt 的白名单环境启动成功，不能声称原错误已修复。

## Root cause
确证的本地缺口是空环境值仍被传递、assist endpoint 不在白名单，以及目标非零退出被当作成功。
原 bootstrap timeout 的根因尚未确认，可能涉及 CLI、网络或用户的路由配置。

## Fix applied
忽略空环境值，防御性保留 CODE_ASSIST_ENDPOINT。非零退出进入 NeedsInteraction；
失败诊断记录目标、版本与去重排序的环境键名，不记录值。保留原生 TTY 错误显示和 argv prompt。
失败记录不因目标已退出而在面板打开时自动释放。

## What we learned
进程 spawn 成功不等于 bootstrap 成功。不能用本机成功推断用户环境故障已解决。
用户复验：同一 shell/cwd 对比直接 codex 与 handoff，核对失败记录的版本、环境键集合；
本地比较代理/assist 值（不要发布凭据），分别测试代理 unset 与空值，保留原始终端报错。
