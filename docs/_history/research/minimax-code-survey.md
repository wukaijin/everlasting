# MiniMax Code(mcode)对比调研

> 调研日期:2026-09-29
> 对象:[MiniMax-AI/minimax-code](https://github.com/MiniMax-AI/minimax-code)(命令 `mcode`,开源部分对应 TUI 0.4.12 源码快照,MIT + 第三方声明,~1.9k stars)
> 目标:对比 mcode 与 everlasting 的架构与能力面,判断哪些设计对本项目有帮助、哪些不应跟进。
> 方法:一手抓取仓库文档(README / docs 全套:tui-capabilities / architecture / verification / open-source-status)+ GitHub API 全树(5051 路径)定位关键实现 + 抓 builtin-defs / sandbox README / automatic-context-compactor / linux-sandbox-utils / MINIMAX_CHANGES.md 六份源文件,对照本仓库 `docs/ROADMAP.md` / `BACKLOG.md` 现状。
> 结论一句话:**帮助在「工程参照实现」而非架构——egress 代理沙箱是本仓库沙箱线余留(bwrap/网络白名单)最完整的落地蓝图,IM channels 是 B10 飞书的架构蓝图,一批语义小件(edit CJK 保真/终止语义/AST 判定)可低成本吸收;进程内无 daemon 形态与云生态不跟进。另:mcode 无 LSP 工具,N15 的「行业标配」证据被削弱。**

---

## 0. TL;DR

1. **mcode 定位 = MiniMax 官方终端 coding agent 的开源部分**(`mcode`,TS/Node 22+/pnpm),TUI / headless(`mcode exec`)/ ACP 三入口;agent 基础设施来自 **vendor 的 pi-mono v0.79.1 fork**(`third_party/pi-mono`,带 60+ 条补丁账本 MINIMAX_CHANGES.md),沙箱来自 vendor 的 sandbox-runtime(bwrap/namespace 路线)。**与 everlasting 同构的部分**:SQLite 持久化、Plan Mode、subagent、skills、MCP、BYOK 多 provider 格式、沙箱三平台、权限审批。

2. **进程模型差异本质**:mcode 是「进程内本地产品」——仓库内运行时不实例化任何 daemon/Desktop HTTP 前门(闭源),云能力(会话 handoff/桌面控制)走闭源云;everlasting 是常驻 Rust daemon(scheduler/群聊/后台任务/远程 PWA 四个面都挂在 daemon 上)。两种生存策略:mcode 靠云生态闭环,everlasting 靠本地常驻的简单可测。

3. **最值得吸收的三件事(§3.1-3.3)**:① egress 代理沙箱完整蓝图(namespace 全断网 + 宿主代理桥开洞 + 凭据假文件替换,seccomp 只拦 AF_UNIX socket 创建)——**直接回应本仓库 sandbox-executor 记录过的痛点**(Landlock ABI v1 无 connect 位、seccomp 无法按路径匹配 sockaddr);② fuzzy edit CJK 保真教训(pi-mono 实锤:NFKC 归一化破坏智能引号/全角字符)+ edit diff 限界(20k 行 167s→0.2s);③ 后台任务终止语义三件(SIGTERM 宽限/父进程死亡守卫/结构化执行事实)。

4. **两条能力线拿到参照蓝图(§3.6-3.7,记 BACKLOG 记注不新立行)**:IM channels(feishu/telegram/wechat 三适配器 + 权限桥 + 问卷桥——B10 飞书立项时的架构参考);通知四事件(turn-complete/turn-failed/permission-required/question-required + unfocused 档 + osc9/osc777/bel 三方法——F6 余留系统级通知的语义清单)。

5. **mcode 有、本项目没有的能力(§3.8-3.10,记 BACKLOG 候选 N18/N19)**:browser 工具(Playwright 型会话级自动化,单工具 24-action 分发;两大 harness 印证,dsh browser-use + mcode browser);task_append(向运行中后台任务/subagent 注入后续工作,activated/steered/duplicate 三态)。会话 fork(/btw 旁路会话 + fork 一等能力)与 dsh 调研重复印证,已有候选 N16。

6. **压缩对照(§3.11)双向印证**:mcode = 每次调用前事件驱动探测(token + 序列化字节 + 归档计划)→ 工具结果归档(可 read 回读)→ 独立 session 生成 checkpoint 摘要 → legacy trim 兜底,**无无进展熔断**——everlasting N12(无进展熔断)在此点领先;mcode 的「归档可回读」(老 tool result 换引用 + 需要时 read 回来)是 C6 截断之外的增量思路。

7. **不建议跟进的(§4)**:插件市场/managed connectors/云账号配额生态(MiniMax 商业闭环)、云权限分类器(外部服务依赖,但其本地 dangerous-patterns 规则值得借)、进程内无 daemon 形态(丢 scheduler/群聊/远程四面)、遥测上传(本地优先不做)。

---

## 1. mcode 架构速览

来源:`docs/architecture.md`、`docs/open-source-status.md`、packages 全树。

### 1.1 分层链路与 vendor 策略

- 链路:`TUI / exec / ACP → CliService → local Applications → Session / Turn / Agent services → Pi / model providers / local tools`。
- **agent 基础设施不是自研**:third_party/pi-mono(earendil-works/pi-mono v0.79.1 fork)提供 agent 循环 / 模型协议 / TUI 基础设施;MiniMax 打了 60+ 条补丁,全部记录在 MINIMAX_CHANGES.md 补丁账本(动机:企业代理环境注入 fetch / Windows 兼容 / 性能与数据保真热修)。
- 沙箱同样是 vendor:third_party/sandbox-runtime(固定 0.0.74-mcode.2),TS 实现 + 原生辅助二进制源码(Linux seccomp helper / Windows Rust 辅助)。

### 1.2 持久化双轨

- canonical 历史 = JSONL(`canonical-history-jsonl.ts` / `jsonl.ts`);会话元数据/索引 = SQLite(better-sqlite3,migration 编号链 0006/0007 建 session-storage 表 + backfill)。
- 有 legacy-migration 体系(旧格式→canonical 转换 + canonical-recovery 启动修复),与 dsh 的世代化迁移纪律同一问题域。

### 1.3 工具面(18 本地 + browser + 云 + MCP + 插件)

本地 18 件(builtin-defs.ts):read / write / edit / bash / grep / glob / todowrite / skill / code_review / ask_user / request_feature_enable / web_fetch / **task 家族五件**(task / task_append / task_query / task_output / task_stop)/ mavis(管理 agent、cron、session、MCP 配置的 agent 自管理工具)。另有 browser 工具(单工具 24-action 分发 + 旧细粒度族)、Matrix 云工具(web_search / 图像生成与理解 / 语音 / 视频)、MCP、官方插件市场。**无 LSP、无 PTY 终端工具**(bash 明确非交互)。

### 1.4 权限与沙箱

- 权限:三层 = 本地规则(bash-ast/split/wrapper-unwrap/write-target/slow-command-scan + dangerous-patterns)+ 云分类器(auto permissions)+ 分类失败回退确认;审批卡可经 IM channel 回流(permission-bridge)。
- 沙箱三平台(§3.1 详述):macOS sandbox-exec(内核级域名 allowlist)/ Linux bwrap+namespace(全断网 + 代理桥)/ Windows 原生 Rust 辅助(ACL/job object/DPAPI)。

### 1.5 压缩(local-context-compaction-v3)

- 触发:每次 LLM 调用前事件驱动探测(`probeBeforeLlm`)——token 超阈 ‖ 序列化字节超阈 ‖ 工具结果归档计划存在。
- 层次:工具结果归档(ToolResultArchiver,有 read 工具时完整归档可回读,否则 trim)→ `compactContext` 决策(llm_checkpoint 摘要 vs 裁剪替换)→ legacy tool trim 兜底。
- checkpoint 摘要用独立 session 生成;strategyVersion 版本化字符串;`hmidOverflowRecovered` 溢出恢复标记。**文件内未见无进展熔断**(everlasting N12 领先点)。

### 1.6 IM channels(本项目 B10 的现成参照)

packages/local-runtime/channels:feishu / telegram / wechat 三适配器 + adapter-registry + access-control-policy/store + channel binding(项目/会话绑定)+ permission-bridge(IM 内权限审批)+ questionnaire-bridge(ask_user 问卷回流)+ 媒体归一化(inbound-media-normalizer)。飞书有独立 permission-card / onboard 流程 / attachments 处理。

### 1.7 会话交互语义(TUI 侧)

- **会话 fork + /btw 旁路会话**:session-system/fork 一等能力(data-capability / display-boundary / side-history-boundary / fork-copy-projection);`/btw` 临时旁路继承「最新完整持久化前缀」(未完成 tool-call 组整组排除),对 /sessions 隐藏,Ctrl+/ 切换主/旁视图。
- **停止语义分档**:Esc 停止 = 取消本进程该会话拥有的后台 bash + subagents(含 task_append 续接);`/clear`/切会话 = 停当前轮但**保留**后台工作;仅本进程拥有的任务被取消。
- 通知四事件 + 终端标题即状态(`Needs approval | 会话名 | MCode`)。
- 输出速度口径:从首个非空 token 起计时,排除首 token 等待与工具执行——度量口径定义得干净。

---

## 2. 逐维度对比

| 维度 | Everlasting | mcode |
|------|-------------|-------|
| 技术栈 | Rust(axum daemon + Tauri)+ Vue 3,单体编译 | TypeScript/Node 22+,pnpm workspace 16 packages + 2 vendor 大件 |
| 进程模型 | 常驻 daemon(:7456,HTTP/SSE/MCP)+ Tauri GUI + 远程 PWA(云中继) | 进程内本地产品,无 daemon;云 handoff 闭源 |
| Agent 基础设施 | 自研 Rust loop,16 关卡,28 builtin tool 编译期注册 | vendor pi-mono fork(agent 循环/协议/TUI)+ 60 补丁账本 |
| 持久化 | SQLite 行存(WAL + FTS5 + turn_trace) | canonical JSONL + SQLite(session-storage)双轨 |
| 工具面 | 28 builtin + skills/commands/workflow + tools stub + MCP | 18 本地 + browser(24-action)+ 云工具 + MCP + 插件市场;无 LSP/PTY |
| 权限 | 5-tier 路径 + 3 档 Mode + A2+ 复合命令拆分 + prefix-grant | 本地规则(AST 级)+ 云分类器 + 回退确认;审批可经 IM 回流 |
| 沙箱 | Landlock + seccomp(拦 AF_INET/6)+ NetPolicy 三态 + 升级闭环 | macOS sandbox-exec 域名 allowlist / Linux bwrap 全断网+代理桥 / Windows 原生辅助;凭据假文件替换 |
| 压缩 | C3+ 摘要 + 0.95 硬卡 + N12 无进展熔断 + 机械丢组 | 归档可回读 → checkpoint 摘要(独立 session)→ trim;无无进展熔断 |
| 多 agent | group_chat 跨模型审议 + dispatch_subagent(worktree 隔离) | task 家族(subagent 与后台任务一体,task_append 注入) |
| 远程形态 | 云中继 + WSS 隧道 + PWA(已交付) | 闭源云(会话 handoff/桌面控制不在仓库) |
| IM 接入 | 无(B10 第四档) | feishu/telegram/wechat 三通道完整落地 |

同构点:SQLite、BYOK 多 API 格式、Plan Mode、skills 渐进披露、MCP、沙箱+权限双层防线、后台任务族——两个不同技术栈的 harness 收敛到同一能力面,互相印证基本盘。

---

## 3. 对本项目有帮助的点(按价值排序)

### 3.1 egress 代理沙箱完整蓝图(sandbox-executor 余留 follow-up 的参照实现)

本仓库 spec 记录过痛点:「seccomp 无法按路径匹配 connect 的 sockaddr 指针、Landlock ABI v1 无 connect 位」,网络白名单因此挂余留。mcode 的解法(读 linux-sandbox-utils.ts):

- **namespace 全断 + 代理桥开洞**:bwrap `--unshare-net` 彻底断网;宿主侧 socat 把 Unix socket 接到宿主 HTTP/SOCKS5 代理;沙箱内 socat 监听 3128/1080 转发回 bind 进来的 Unix socket;环境变量 `HTTP_PROXY` / `GIT_SSH_COMMAND`(SSH 走 SOCKS)/ JVM agent jar 注入。
- **域名过滤在宿主代理层**:Linux namespace 全有/全无的妥协被如实记录(与 macOS 内核级域名 allowlist 的分工差异写明)——白名单语义不丢,只是执行点从内核挪到代理。
- **seccomp 只拦 `socket(AF_UNIX, ...)` 创建**:防沙箱内命令绕过代理自建通道;局限如实记录(不拦已继承 fd / SCM_RIGHTS)。
- **凭据保护**:credential-extract(AWS sigv4 对等)+ credential-mask-env/files + **假文件整体替换**(maskedFileBinds)——.git/hooks、.git/config、dotfiles 等 deny 路径用 ro-bind/tmpfs//dev/null 掩盖。
- 对应 everlasting 余留「bwrap 可选增强档 + 网络白名单/egress 代理」:方案轮廓、取舍记录、已知局限都是现成的;NetPolicy 三态(Block/BindOnly/AllowAll)与代理桥可拼(AllowAll 之外的档,代理桥即是「白名单断网」的落地形态)。**动工时按 C.3 记注引用本调研,不必重新摸索**。

### 3.2 fuzzy edit CJK 保真教训(立查项)

pi-mono 补丁账本实锤:一次 fuzzy edit 会把**整个文件**做 NFKC/ASCII 归一化,破坏智能引号、全角字符等 CJK 内容;修法 = 只替换「映射回原文的精确 span」+ 保真守卫防吞兼容字符。**everlasting 的 edit_file 有 fuzzy 匹配路径的话应立即自查同类问题**(本项目用户群 CJK 内容占比高,风险面同构)。

### 3.3 后台任务终止语义三件(pi-mono 补丁,低吸收)

1. **SIGTERM → 宽限期 → SIGKILL**:让脚本 trap 清理逻辑能执行(对照 everlasting 后台 shell 停止路径)。
2. **父进程死亡守卫**(detached IPC lease):宿主崩溃后自动回收后台进程组,Unix sh gate + Windows 精简启动器——对照 everlasting 孤儿清理(目前是 daemon 侧 sweep,「宿主死亡」视角不同)。
3. **结构化执行事实**:exit/signal/timeout/cancellation/部分输出全保留,仅 exit 0 为成功——tui-capabilities 明文契约,对照 C6 截断的「事实保留」口径。

### 3.4 bash 判定 AST 深化(A2+ 增量参照)

mcode 权限层的 bash 判定文件族:bash-ast / bash-split / **bash-wrapper-unwrap**(剥命令包装层)/ bash-write-target / **slow-command-scan**(慢命令标记)/ windows-native-delete(Windows 原生删除路径)。比 everlasting A2+ P1+P2(复合命令拆分 + 写重定向检测)深一层;云分类器不跟进,但其**本地 dangerous-patterns 规则集**可作 A2+ 词表扩充参照。

### 3.5 并行批次取消配对补齐(agent-loop 语义小件)

pi-agent-core 补丁:工具 hook `terminateAgent` 显式终止 agent 时,**并行批次中未执行的调用补合成错误结果以保持历史配对**——tool_use/tool_result 配对完整性在取消路径上的保障。另有 `steerBatch`(一批本地消息在一个 provider hop 内消费)与 everlasting F1 续轮批量注入同语义,方向互证。

### 3.6 IM channels 架构蓝图(B10 立项时读)

三通道 + adapter-registry + access-control 独立策略/存储 + channel binding(项目/会话绑定与路由)+ permission-bridge(IM 内审批权限卡)+ questionnaire-bridge(ask_user 问卷过 IM)+ inbound-media-normalizer。B.2 已记注 B10 收窄首发形态为「任务完成通知」——届时按此蓝图评估,飞书 permission-card / onboard / attachments 三件有专门适配。

### 3.7 通知四事件语义清单(F6 余留系统级通知的参照)

mcode notifications 契约:`events: [turn-complete, turn-failed, permission-required, question-required]`(省略=全开,[]=禁用)+ `when: unfocused|always|never` + `method: auto|osc9|osc777|bel`;前台焦点默认抑制;通知去重含会话标识。F6 余留增强(系统级通知/unread 持久化/等待态心跳)立项时,这份四事件 + 三档时机 + 多方法回退的清单可直接抄作业。

### 3.8 browser 工具(记 BACKLOG 候选 N18)

会话级 Playwright 型自动化:单工具 `browser` 24-action 分发(inspect/query/navigate/click/fill/wait/screenshot/upload_files…)+ 旧细粒度族;双形态(Electron 内嵌面板 + 原生无头 Chrome 隔离 profile);**不支持任意 JS 执行**;快照分页(snapshotId/offset);每回合自动可视化观察图片上限 4 张(token 治理);敏感操作前用户确认 + `safety.requiredNextTool` 强制下一步。dsh(browser-use/computer-use)与 mcode(browser)两大 harness 均有——**候选证据比 dsh 调研时更足**。

### 3.9 task_append:向运行中任务注入后续工作(记 BACKLOG 记注)

task 家族五件里最有增量的一件:`task_append` 向已有后台任务/subagent 发送后续工作(activated/steered/duplicate 三态),配合 task_output 字节偏移读。对照 everlasting:L1 后台 shell 的 APPEND 是**用户通知面**,dispatch_subagent 无中途注入;agent 侧「给运行中的 worker 追加指令」语义是空白。

### 3.10 会话 fork 与 /btw 旁路会话(N16 证据加强)

mcode 把 fork 做成一等能力(fork/ 目录 + ACP 下发 fork 语义)+ `/btw` 旁路会话(继承最新完整持久化前缀、未完成 tool-call 组整组排除、继承权限模式、对会话列表隐藏)。N16 立项时「种子 = 截止 seq + 权限模式」的语义可直接参考。

### 3.11 压缩双向印证

- mcode 值得看的点:①**工具结果归档可回读**(老 tool result 归档为引用,agent 需要时 read 回来)——比 everlasting C6(单次输出超限截断)多一个「历史结果瘦身不销毁」的层次;②序列化字节阈值与 token 阈值并列触发(网络/序列化成本的兜底信号);③strategyVersion 版本化字符串 + hmidOverflowRecovered 恢复标记。
- everlasting 领先点:**N12 无进展熔断**(连续摘要 Applied 但水位未推进 → 粘性跳过摘要直达机械)mcode 没有——C3+ 压缩 + 0.95 硬卡的收口语义仍是本仓库更完备。

---

## 4. 不建议跟进的

| 项 | 理由 |
|----|------|
| 插件市场 / managed connectors / 云账号配额 | MiniMax 商业闭环(短期 token lease broker / 内部 registry / 发布管控),与本地优先单机产品无关 |
| 云权限分类器(auto permissions) | 依赖其云端服务;但本地 dangerous-patterns 规则值得借(见 §3.4) |
| 进程内无 daemon 形态 | mcode 的云 handoff/桌面控制靠闭源云补齐;everlasting daemon 常驻挂着 scheduler/群聊/后台任务/远程 PWA 四个面,进程模型不因它动摇(两模型各有取舍,与 dsh 调研结论一致) |
| pi-mono vendor 模式 | everlasting 单体自研无 vendor 需求;但其 MINIMAX_CHANGES.md「补丁账本 + 上游同步工作流」的纪律本身是好的(本仓库 spec 沉淀已是等价物) |
| canonical JSONL + SQLite 双轨持久化 | 比 everlasting 纯 SQLite 多一套格式迁移/恢复体系(legacy-migration + canonical-recovery 一整个目录);无 FTS5/turn_trace 等价物,没有理由引入 |
| 遥测/诊断上传 | 本地优先不做;其「诊断 allowlist + 计数-only + 排除 prompts/堆栈/URL」的克制设计倒是与本地优先气质相合,仅记一笔 |

另:开源部分是「源码快照」而非开发仓库(128 commits,内部 Git 历史不进入,版本号一致不证明可复现 npm 包——open-source-status.md 自述)——**只宜读设计,不宜引入依赖**(TS 生态,技术上也无法直接复用)。

---

## 5. 来源

- 仓库:<https://github.com/MiniMax-AI/minimax-code>(main 分支,2026-09-18 导入快照,对应 TUI 0.4.12)
- 能力边界:<https://github.com/MiniMax-AI/minimax-code/blob/main/docs/tui-capabilities.md>
- 架构:<https://github.com/MiniMax-AI/minimax-code/blob/main/docs/architecture.md>
- 开源状态:<https://github.com/MiniMax-AI/minimax-code/blob/main/docs/open-source-status.md>
- 验证记录:<https://github.com/MiniMax-AI/minimax-code/blob/main/docs/verification.md>
- 工具清单:<https://github.com/MiniMax-AI/minimax-code/blob/main/packages/agent-tools/src/desktop/builtin-defs.ts>(+ builtin-browser-defs.ts)
- 沙箱:<https://github.com/MiniMax-AI/minimax-code/blob/main/third_party/sandbox-runtime/README.md> + src/sandbox/linux-sandbox-utils.ts
- 压缩:<https://github.com/MiniMax-AI/minimax-code/blob/main/packages/local-runtime-v2/src/service/turn-system/compaction/automatic-context-compactor.ts>
- pi-mono 补丁账本:<https://github.com/MiniMax-AI/minimax-code/blob/main/third_party/pi-mono/MINIMAX_CHANGES.md>
- 全树:GitHub API `git/trees/main?recursive=1`(5051 路径,经 grep 定位 channels/permission/compaction/session-system/fork 模块)
