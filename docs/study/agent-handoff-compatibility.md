# 跨 Agent 交接兼容性调研

日期：2026-09-22。状态：实施前调研记录，非当前支持清单。

本机适配曾于 2026-09-22 覆盖 Codex、Cursor、OpenCode、Kimi、Pi、Gemini、Copilot、
Grok、DimAgent（已移除；历史）；2026-09-25 起按用户产品决策收缩为 Codex、Cursor、Kimi，
其余 6 种的适配已移除（本页保留的调研结论仍可作为重新适配的依据）。
范围、限制和统一验收状态见 [实施记录](../impl/agent-session-handoff-plan.md)。

关联：[设计入口](../design/agent-session-handoff.md)、
[架构契约](../design/agent-session-handoff-contract.md)、
[实施计划](../impl/agent-session-handoff-plan.md)。

## 结论

**交接包方案可以扩展到其他 Agent，但来源端和接收端必须分别验收。**
本次新增研究 Claude Code、OpenCode、Kimi Code、Pi、Gemini CLI、GitHub Copilot CLI、
Qwen Code、Goose、Amp、Factory Droid 共 10 个 Agent。

优先扩展 OpenCode 和 Claude Code；Kimi 与 Pi 接口适合继续验证；
Gemini、Copilot、Qwen、Goose 可进入下一批；Amp 和 Droid 保留独立限制。
这是建议优先级，不改变已提议的 Codex→Cursor 首版范围。

本次只执行版本/help 与 3 个 ACP initialize；没有请求模型推理、恢复已有会话、
发送 prompt、执行代码修改、安装新 Agent 或公开任何会话。
之前的 Codex 历史读取和 Cursor 握手证据继续见主设计文档。

## 如何阅读支持矩阵

- **来源端**：取得准确 session 的可导出记录，转为统一交接包。
- **接收端**：创建目标新会话并投递交接包；已有 resume 仅说明同产品续接能力。
- **协议能力**：方法存在或握手声明；不证明默认权限、TUI 恢复和跨 Agent 交接成功。
- **可接入**：已有明确官方机制，可以进入实现验证；不等于 Con 现在已经支持。
- **有条件**：依赖特定存储版本、加载会话、用户导出或其他尚未实测的组合。

| Agent              | 来源端依据                                        | 接收端依据                                            | 判断 / 本次证据                                                    |
| ------------------ | ------------------------------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------ |
| Claude Code        | 官方 SDK listSessions/getSessionMessages          | 原生 CLI 初始 prompt、--session-id、--resume；SDK     | 双向可接入；官方文档，本机 PATH 未发现 claude                      |
| OpenCode           | export JSON；服务端 session/messages              | 创建 session、发送 message；TUI --session/--prompt    | 双向可接入；本机 1.18.27 help + 官方文档                           |
| Kimi Code          | export ZIP；服务端 messages/transcript            | 原生 session 续接、prompt；ACP                        | 双向有条件可接入；本机 2.0.1 help 与 ACP 握手                      |
| Pi                 | 受管 RPC get_messages；精确 session 文件加载      | RPC prompt/new_session；原生初始 prompt、--session-id | 双向有条件可接入；本机 0.86.1 help + 官方 RPC 文档                 |
| Gemini CLI         | 文档化本地会话存储；ACP load 能力                 | 初始 prompt、--session-id；ACP                        | 接收可接入，既有历史导出需版本适配；本机 0.60.0 help 与握手        |
| GitHub Copilot CLI | SDK getEvents；本地 Markdown 导出                 | 新 session ID、初始 prompt；ACP/SDK                   | 双向可接入，SDK 加载副作用须验收；本机 1.0.80 help 与握手          |
| Qwen Code          | /export json/jsonl；会话恢复机制                  | ACP/serve 的提示接入                                  | 双向有条件可接入；仅官方文档，未安装探测                           |
| Goose              | session export JSON/Markdown                      | 新交互会话；指定会话续接                              | 双向有条件可接入；仅官方文档，自动初始提示组合待测                 |
| Amp                | threads markdown/export                           | CLI 会话输入与 thread continue                        | 可导出；本机接收路径待验证，远程 orb 不满足同 worktree；仅官方文档 |
| Factory Droid      | listSessions 仅元数据；受管 stream 可记录未来历史 | createSession/resumeSession/stream                    | 接收可接入；任意已有会话的完整历史读取未证实；仅官方文档           |

“export/import”默认是该产品自己的数据格式；不把 OpenCode、Goose 等的原生 import
作为通用跨 Agent 格式。Con 仍然发送带来源说明的上下文。

## 逐项依据与限制

### Claude Code

官方 Agent SDK 提供磁盘会话发现和消息读取接口，可避免 Con 直接维护 Claude 私有数据库解析。
原生 CLI 可以传初始提示、指定 UUID、新建或恢复会话。
优先路径：SDK 只读导出 → Con 交接包 → 原生 CLI 新会话。
SDK 的 Node/Python 依赖需单独评估打包；Rust 工程不应因一次调研直接引入运行时。
本机未发现 claude 命令，不代表用户没有其他路径安装；本次没有 CLI/SDK 实测。

来源：[会话 SDK](https://code.claude.com/docs/en/agent-sdk/sessions)、
[CLI 参数](https://code.claude.com/docs/en/cli-reference)。

### OpenCode

官方服务端包含 session 创建、历史消息读取、消息发送、状态和 abort；
CLI 支持按 session ID 导出 JSON，本机 help 也确认 export、--session、--prompt 和 ACP。
它适合第一个新增适配器：Con 已有 OpenCode 终端目标控制，导出有独立命令。
首次 spike 使用精确 ID 导出并验证 sanitize 后的覆盖范围，不运行 import 来伪造外部会话。
服务端必须绑定明确地址、项目与认证；一个新服务实例的状态不证明另一 TUI 已停止。

来源：[Server](https://opencode.ai/docs/server/)、[CLI](https://opencode.ai/docs/cli/)。

### Kimi Code

本机为 Kimi 2.0.1，当前官方文档是新版；不能继续按旧版 Python CLI 的 wire 参数设计。
export ZIP 可以用于离线来源适配，但本机 help 显示默认还会打包全局诊断日志。
使用该路线时必须指定精确 session ID 与 --no-include-global-log，且只提取允许的会话文件。
ZIP 解包需限制大小和路径；禁止把整包直接传给目标 Agent。

服务端 messages/transcript 提供历史访问，但文档明确冷会话读取可能恢复会话。
因此该读取路线标记为 loads_session，不能在 Preflight 悄悄触发。
ACP 握手只证明当前安装支持协议，未验证加载或交接。

来源：[Sessions](https://www.kimi.com/code/docs/en/kimi-code-cli/guides/sessions)、
[Server API](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/server-api.html)，以及本机 help。

### Pi

RPC 提供 get_state、get_messages、new_session、switch_session、prompt。
可在 Con 托管 RPC 时准确记录 ID；手动 TUI 的来源需要先识别精确 session，再验证读取路径。
get_messages 的结果应按实际返回覆盖范围标记，不承诺包含所有压缩前记录或所有分支。
本机 CLI 支持 --session-id 和 --session；RPC 与原生 TUI 续接仍须组合测试。

取消不能只调用 abort：官方说明待执行消息队列可能继续运行，应先处理 clear_queue，
再等待停止，并检查独立 bash 操作。这是适配器语义，不应塞进通用状态机的固定命令。
RPC 是 JSONL 协议，不应假定它是 ACP 或通用 JSON-RPC。

来源：[Pi 官方 RPC 文档](https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/coding-agent/docs/rpc.md)，
以及本机同版本 docs/rpc.md 和 CLI help。

### Gemini CLI

官方说明会话自动保存在项目对应的 chats 目录；文档化位置不等于跨版本稳定的导出 schema。
可采用版本化只读存储 reader，或在能力确认后使用 ACP 加载回放；禁止猜目录 hash 算法和选最近会话。
本机 CLI 有 --session-file、--session-id、--resume 与交互初始提示。
session-file 是 Gemini 自身格式加载入口，不是 Con 交接包导入协议。

本机 ACP 声明 loadSession=true，但没有声明 session list；会话发现不能凭 ACP 名称假定存在。
来源中可能包含思考字段，归一化时须剔除，不向目标传递。

来源：[Gemini 会话管理](https://geminicli.com/docs/cli/session-management/)，以及本机 help/握手。

### GitHub Copilot CLI

官方 SDK 当前源码的 getEvents() 读取 session.getMessages 返回的事件；需验证精确版本与
恢复连接的生命周期影响。CLI 也提供本地 Markdown 导出，可作为手动来源路径。
本机支持 --session-id、-i、--resume 与 ACP，并已通过 initialize。

必须明确使用本地 file 导出：当前 /share 默认可能生成 GitHub 分享链接，不能用于隐式导出。
禁止用 gist 分享代替本地交接。ACP 的握手不证明 SDK 与 TUI 同时操作会话是安全的。

来源：[CLI 官方参考](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)、
[SDK 会话实现](https://raw.githubusercontent.com/github/copilot-sdk/main/nodejs/src/session.ts)。

### Qwen Code

官方命令支持 /export json、jsonl、md 和会话恢复，serve 文档提供 ACP 桥接路线。
来源优先 JSON/JSONL；当前 HTML 导出依赖外部渲染资源，不适合机器交接。
不要为了获得摘要调用 /summarize：官方注明它是压缩历史的别名，会改变源历史。
自动导出、指定 native ID、原生终端恢复需安装后的版本矩阵验证；slash 命令存在不等于有独立只读 CLI。

来源：[官方命令](https://github.com/QwenLM/qwen-code/blob/main/docs/users/features/commands.md)、
[serve 文档](https://qwenlm.github.io/qwen-code-docs/zh/users/qwen-serve/)。

### Goose

官方提供 JSON/Markdown 会话导出和按 ID 续接机制，具备来源适配基础。
原生新会话接收交接指令可采用交互输入；自动启动与首次投递的组合尚未验证。
导出可能包含设置和扩展数据，必须筛选；不把原生 import 当作外部 Agent 权限与配置迁移。

来源：[Goose 会话管理](https://goose-docs.ai/docs/guides/sessions/session-management/)。

### Amp

官方 CLI 可导出 Markdown/JSON；完整 JSON 有创建者权限限制，降级 Markdown 要标记覆盖范围。
threads continue 可以续接，但远程 orb 线程仍在远程机器执行。
本设计首版要求同一宿主机 worktree，因此必须区分 local_cli 与 remote_orb。
官方产品中的 handoff 是 Amp 内部功能，不能据此宣称支持任意外部 Agent。
接收端须单独核实本地新会话启动、ID 获取和包读取；本次未实测。

来源：[Amp Threads](https://ampcode.com/docs/threads)。

### Factory Droid

官方 SDK 提供 createSession、resumeSession、stream；listSessions 只读本地元数据。
本次未找到该文档中对任意已有会话的完整 transcript 只读接口，不能把 listSessions 当作历史导出。
Con 从创建时就托管的会话可保存 stream 记录；历史覆盖从接入时起，不追溯为完整历史。
resumeSession 会恢复原工作目录和设置，不能用它替代接收外部上下文的新会话。
建议先做接收适配，已有会话来源保持未证实。

来源：[Droid SDK](https://docs.factory.ai/sdk/typescript)。

## 本机探测记录

| Agent    | 版本    | 执行内容                                                 | 结果                                              |
| -------- | ------- | -------------------------------------------------------- | ------------------------------------------------- |
| OpenCode | 1.18.27 | --version、--help、export --help                         | 确认 JSON 导出、会话/TUI/服务入口；未实际导出     |
| Kimi     | 2.0.1   | --version、--help、session/export --help、acp initialize | 成功，loadSession、list/resume/close 等能力已声明 |
| Gemini   | 0.60.0  | --version、--help、--acp initialize                      | 成功，loadSession=true；未声明 session list       |
| Pi       | 0.86.1  | --version、--help、随安装包的 RPC 文档                   | 确认 session 参数与 RPC 命令；未启动 RPC 会话     |
| Copilot  | 1.0.80  | --version、--help、--acp initialize                      | 成功，loadSession=true，声明 session list/close   |

三个 ACP 探测在独立临时目录运行，仅发送 initialize，返回后关闭所启动的进程。
没有发送 authenticate、session/new、session/load、session/prompt。
启动 CLI 可能自行做初始化或版本检查，不能声称整个用户目录零写入；没有主动修改配置。
这些是已安装版本基线，不能作为最小支持版本。

其他 Agent 未在本次探测的 PATH 中发现或未安装验证。Con 已有图标的
Grok、Mimo、Qoder、Hermes、Kiro、Cline、Kilo、DimAgent（已移除；历史），以及 Herdr 不在本轮深度研究范围。
它们保持 Unknown；尤其 Herdr 是终端编排工具，不应仅凭图标作为同类会话来源。

## 对设计的修订

1. 将单一 read_history 布尔能力拆成机制、覆盖范围、生命周期影响；未知不能视为只读。
2. session 发现、创建、读取、接收、停止分别声明；ACP loadSession 不保证 list 或历史完全回放。
3. 来源与接收独立验收；一个来源 reader 可复用于多个目标，无须写 N×N 个转换器。
4. 原生文件 export 的元数据/日志/权限必须过滤；云端分享接口不能作为自动导出降级。
5. 按实际运行位置校验：同产品也可能在本机、远程 daemon 或云端运行。
6. 新增适配器默认禁用，只有版本化 fixture 和真实接续通过后才标记可自动交接。

## 验证优先级

| 批次   | 建议                         | 原因与门槛                                                 |
| ------ | ---------------------------- | ---------------------------------------------------------- |
| 原首版 | Codex→Cursor                 | 保持已有设计目标，先跑通真实接续                           |
| 扩展 A | OpenCode、Claude Code        | 明确历史接口；已有 Con 终端控制基础，Claude SDK 打包先评估 |
| 扩展 B | Kimi、Pi                     | Kimi 离线导出与冷加载边界；Pi 队列取消与会话覆盖范围       |
| 扩展 C | Gemini、Copilot、Qwen、Goose | 版本化读取、本地导出、TUI/协议身份一致性                   |
| 专项   | Amp、Droid                   | Amp 执行位置；Droid 既有历史来源未证实                     |

每个来源做固定历史→交接包契约测试，每个目标做包→新会话接收测试；
另外实测至少 OpenCode→Claude、Kimi→Codex，以及一条 Gemini 或 Copilot 的边。
有条件能力不能靠中间包测试自动升级为端到端支持。

## 记忆检索

Nowledge MCP，query=`Agent session 交接 Kimi Claude OpenCode ACP`，默认 normal，
响应 retrieval_mode=strict、scope=default，无额外过滤，limit=1，结果为空。
因此本轮结论基于上述官方资料与本机探测，没有相关旧决策可复用。
