# Agent Handoff 弹窗复用导致状态过期

## 发生了什么

2026-09-23 验收 Handoff 弹窗时发现：通过 CLI 取消一个进行中的 handoff job 后，再次触发 `handoffs.open`，弹窗仍显示取消前的 job 状态（"Creating target session…"），且没有任何可操作按钮，用户被卡在过期视图里。

## 根因

`open_agent_handoff` 复用已存在的弹窗窗口（`handoff_window`），只调用 `activate_window()` 置前。而 `HandoffDestinationPanel` 的 source session、active job、目的地列表全部在打开时（`load()`）绑定一次，之后没有刷新路径。窗口复用 = 永远展示首次打开时的快照。

## 修复

再次触发 Handoff 时不再复用旧窗口：`remove_window()` 关掉旧弹窗后全新打开，面板重新执行三路并行加载（agents / sessions / binding），保证看到的永远是当前状态。同期把弹窗 UI 重写为 Settings 窗口风格（透明 titlebar + 44px 自定义 header、10px group label、rounded-12 半透明卡片、px16/py12 行），并精简文案（"Session not detected"、"Source work stopped"）；已有 Agent Tab 行改为以「Tab N · Agent」为主标题、Tab 标题为辅助信息，避免过期缓存的 Tab 品牌名误导。

## 学到什么

- 凡是"打开时绑定一次"的面板，窗口复用就是状态过期 bug 的温床。要么面板监听数据变化持续刷新，要么打开策略改为关旧开新——后者对一次性对话框更便宜也更安全。
- 验收弹窗类 UI 时，"关闭再打开"和"再次触发"是两条路径；只测其中一条会漏掉复用分支。
