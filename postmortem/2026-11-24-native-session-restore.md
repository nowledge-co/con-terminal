# Codex 会话路径与 Claude 全局配置恢复

macOS 的恢复命令可能在启动 Codex 前就报错，或让已使用按量 API key 的 Claude 用户再次登录。

## 原因

Codex 的 SessionStart 可以返回会话 UUID 而省略 transcript_path。con 接受了这种记录，恢复时却把路径当作必填项。Claude 的默认配置和显式设置 CLAUDE_CONFIG_DIR 的配置可以指向同一目录，但选择不同的全局配置文件，API key 的批准状态也可以不同。con 原先在恢复子进程中总是设置这个变量。

## 修复

Codex 缺少路径时，将经过校验的 UUID 和已保存的 CODEX_HOME 交给原生 resume 命令。有路径时继续校验文件、目录范围和会话 ID。Claude 的新记录保存配置目录变量是否显式设置。旧记录指向默认目录时，仅在恢复环境显式选择同一目录时沿用该覆盖。con 无需读取或迁移密钥。

## 验证经验

恢复测试需要覆盖 hook 允许省略的字段、旧记录，以及使用同一目录但环境变量设置方式不同的启动。通过隔离终端检查原生子进程实际收到的参数和环境，可以验证这些差异。

在临时 HOME 中用真实原生 CLI 验证了两个边界。Codex 恢复了只有 UUID 的测试记录，传入不存在的 UUID 时返回找不到会话。Claude Code 2.1.331 的两份测试配置使用相同的虚拟 API key，分别记录为允许和拒绝。两种方式都能打开原对话并显示 API Usage Billing。发送第一条测试消息时，显式配置报出 Not logged in · Please run /login，恢复默认配置后则尝试连接指定的本机 API 地址。验证没有使用真实密钥或调用付费模型。
