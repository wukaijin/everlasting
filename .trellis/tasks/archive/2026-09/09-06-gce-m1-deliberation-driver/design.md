# Design — GCE-M1 群聊审议驱动

## 架构与边界

三层各司其职(立项讨论定形):daemon 群聊原语(平台,零改动)、`scripts/group-chat-run.mjs`(引擎,本任务核心)、`.agents/skills/group-chat/`(指引门面,零逻辑)。**daemon 零改动**是本任务的红线——所有能力经既有 HTTP API 组合;若实现中发现 API 缺口,回头修 API 层并独立成 commit,不在脚本里造 workaround。

## 数据流:`run` 的状态机

```text
resolve-project ──► build-body ──► create_session ──► agent/chat(异步发,不依赖其响应体)
      │                 │                                   │
  list_projects     预设+override                    poll loop(10s 间隔,list_sessions)
  按路径匹配             合并                          │
  miss→create_project                                  ├─ busy=true → 打一行进度,继续
                                                       ├─ busy=false + stop_reason≠null → 终态
                                                       └─ 超时/SIGINT → cancel_chat → 等 ≤60s 落终态
                                                            │
                                                       load_session → discussion_summary + messages
                                                            │
                                                       渲染转录 markdown → out/(或 --out)
                                                            │
                                                       退出码:0 group_chat_end / 2 max_rounds
                                                              3 cancelled / 4 error
                                                       (--cleanup 且成功 → delete_session)
```

要点:

- **chat 请求与轮询并行**:`agent/chat` 的 HTTP 响应要等整场编排(5-15min),脚本 fire 后不 await 响应体(超时/连接断不视为失败),终态判定唯一来源是 `list_sessions` 的 `busy`/`stop_reason`(GC1/GC2 契约,编排级粒度天然防轮间空隙误判)。
- **全程不挂 SSE 连接**:这是语义决策不是实现偷懒——SSE 订阅者存在会把权限 ask 翻回 120s 等待(GC3),无人值守审议必须保持 8s 快拒。turn-smoke 走 SSE 是因为它是有人值守的冒烟;两条路线语义不同,不共享。
- **中断收尾**:SIGINT/超时 → `cancel_chat`(停编排、保 session;区别于 turn-smoke 的 delete_session 腰斩)→ 轮询等 `stop_reason=cancelled` 落定(≤60s 兜底)→ 导出部分转录 → exit 3。
- **dry-run**:走完 resolve-project(只读)与 build-body,打印将发出的 `create_session` body 与首条 chat wire,不发起任何写请求。

## 内省子命令 ↔ API 映射

| 子命令 | HTTP | 输出 |
|---|---|---|
| `projects` | `POST projects/list_projects` | 路径 + project_id 表 |
| `models` | `POST providers/*`(目录端点) | provider → 模型 catalog key 列表 |
| `presets` | 无(本地常量) | 预设名 → participants(name/model/persona 摘要)+ 可覆盖字段说明 |

## 预设草案(内置常量;persona 全文实现时打磨)

以两场 live 验证过的阵容为基线:

- **review 评审团**(= 两场 live 实跑阵容,已验证产出质量):moderator 默认 MiniMax-M3;林澈-架构/glm-5.3、苏晚-产品/GLM-5.3-Flash、赵拓-后端/deepseek-v4-flash。
- **arch 架构决策**:精简双人 + moderator(架构 + 实现两视角,议题聚焦单决策点)。
- **retro 复盘**:主持 + 当事视角 + 局外视角(防同温层)。

模型引用一律用 catalog key;`models` 内省保证 LLM 拿到的是当前真实清单,预设里模型失配时给明确报错与替代建议(而非静默降级)。

## 转录格式

对齐既有三份 `out/group-chat-*.md`:文件头 metadata(session id、阵容、时长、stop_reason、summary)+ `seqN **speaker**: 正文` 逐条。数据源 `load_session` 的 messages。**落点解析钉死为 everlasting 仓库根的 `out/`(按脚本自身位置推导,不是 CWD)**——嵌套消费时外层 agent 的 cwd 是别的项目,CWD 相对会把转录散落出去;`--out` 显式覆盖;结束时 stdout 打印转录绝对路径(外层 agent 从后台 shell 输出里拿)。

## 嵌套消费:daemon 单聊「套娃」(AC5 最终验收)

外层 = 普通 chat loop(非 everlasting 项目),内层 = 独立 session 的群聊编排;连接方式 shell → HTTP,两层进程/API 解耦——不是 `dispatch_subagent` 式 loop 嵌套(第二场 live 判词),无共享循环状态。daemon 多 session 并发是既有设计(GUI/remote PWA/F2 定时任务共存),但「外层 busy + 内层编排同跑」作为 live 验证点之一。

外层消费模式(v1,不扩 M1 范围):

- run 以**后台 shell** 起(session 53 基础设施),外层轮询 shell 输出拿转录绝对路径与退出码;
- `--detach` + `status <session_id>` / `result <session_id>` 异步形状**划 M2**——正是其 `start/status/result/cancel` 四工具的 CLI 前身,本测试构成 M2 的需求验证。
- `--project` 默认当前目录:外层视角下 = 外层项目自身 = 审议对象代码库,证据基地自动正确。

## 兼容与回滚

纯增量:一个脚本 + 一个 skill 目录 + 两处文档。无 daemon/GUI/DB 改动,回滚 = 删文件。M2 时 MCP 工具直接 import 脚本的 build-body/轮询/转录函数(实现时把可复用逻辑放纯函数区,CLI 只是薄壳——这是为 M2 预留的唯一结构约束)。

## 权衡记录

- **轮询 10s 间隔**:lifecycle 是编排级粒度(轮间不回落),10s 足够;更细的发言级进度需要 SSE,明确不做(Q2 决策)。
- **退出码 2/3/4 留 1 给脚本自身错误**(参数/网络/daemon 不在线),与 stop_reason 四值区分开。
- **--cleanup 只作用于成功路径**:中断现场保留是 post-mortem 语义,不给开关破坏它。
