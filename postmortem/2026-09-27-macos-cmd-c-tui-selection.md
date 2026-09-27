# macOS 下 ⌘C 无法复制 Codex TUI 中拖选的回复

## 发生了什么

用户反馈 Claude 的限制过严，只得回到 Codex；随后在 Con Beta 122 中发现：普通 shell 文本可以拖选并用 ⌘C 复制；在 Codex TUI 中拖选回复时，界面显示 `ctrl+c copy`，但按 ⌘C 后剪贴板内容不变。在另一个 shell 标签页中粘贴仍正常。

## 根因

Codex 开启鼠标报告并自行管理拖选，因此 Ghostty 没有终端选区可供 Con 读取。Con 的 macOS ⌘C 处理逻辑在 `has_selection()` 为 false 时仍会消费快捷键，Codex 因而收不到它用于复制选区的 Ctrl+C。

## 修复

Con 记录一次被 TUI 接管的左键拖选。如果拖选结束后 Ghostty 没有选区，紧接着的普通 ⌘C 会向 TUI 发送一次 Ctrl+C。新的左键或右键操作、滚动或其他按键会清除待处理状态；终端失焦或关闭时会取消整个手势，避免切换标签后误发 Ctrl+C。普通 shell 拖选与 Ghostty 自身选区继续使用 Con 原有的复制路径；「编辑」菜单中的「复制」也使用同一回退逻辑。

Codex 本身提供 `tui.raw_output_mode = true` 配置（写入 `~/.codex/config.toml`），也可在会话中使用 `/raw` 或 `Alt+R` 切换。该模式使用更便于终端选区复制的 scrollback，但会改变 TUI 的交互方式。Con 的修复保留 Codex 默认交互和用户的 ⌘C 习惯。配置见 [OpenAI Docs](https://developers.openai.com/codex/config-reference)。

## 经验与限制

终端中可见的选区可能由终端管理，也可能由交互程序管理。复制快捷键不能只检查终端自身的选区。

Con 无法判断任意开启鼠标报告的 TUI 是否真的选中了文本。因此，在这类 TUI 中拖动鼠标且 Ghostty 没有选区后，紧接着按 ⌘C 也可能把 Ctrl+C 发给并不支持该复制操作的程序。新按键、左键或右键操作、滚动及终端失焦会清除此状态，降低误触发机会。
