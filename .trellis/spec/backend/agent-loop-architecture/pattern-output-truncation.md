# Pattern: 工具大输出截断契约(C6,2026-08-30)

> 来源:任务 `08-30-c6-output-truncation`(PRD/design/review/implement 走
> `.trellis/tasks/08-30-c6-output-truncation/`)。实现
> `tools/tool_output.rs`(契约模块)+ shell / background_shell / read_file /
> web_fetch / grep 五处迁移。ROADMAP 第三档 C6「大输出截断统一」收口。

把「每个工具自己截断、各自为政」变为统一契约:**每次截断必须自带一条
恢复通路,且标记 machine-parsable、全工具同一格式**。统一的对象是
契约,不是上限数字 —— 数字语义各异(行 vs 条目 vs 字节),per-tool 保留,
但集中一张常量表(`tool_output.rs` 头部)。

## 三种 sanctioned 恢复模式

| 模式 | 机制 | 适用(现状消费方) |
|------|------|------------------|
| **A 落盘 + 路径** | 全量写 `<app_data_dir>/outputs/<session_id>/<uuid>.txt`,标记给绝对路径,LLM 用 `read_file` offset/limit 分页读 | shell、background_shell、web_fetch(>100KB)—— 一次性、不可重放的结果 |
| **B range 参数** | offset / limit / head_limit,标记里写明参数名 | read_file、grep(行级 + head_limit)—— 可重复执行的查询 |
| **C 收窄提示** | 提示收窄查询 | glob(参照实现;仅当结果可由更窄查询完整重放时允许) |

**web_fetch 特例(禁重取)**:模式 A 是 web_fetch 大内容**唯一** sanctioned
恢复方式 —— 两次 fetch 可能观察到不同内容,offset 重取会静默漂移;落盘
一次、切片多次才诚实。

## 统一标记格式(R2)

```text
<truncated: omitted N of M bytes | full output: <abs-path> | recover: read_file with offset/limit>   ← 模式 A
<truncated: omitted N of M bytes | recover: <param hint>>                                            ← 模式 B
<truncated: omitted N of M matches | recover: narrow the pattern>                                    ← 模式 C
<truncated: omitted N of M bytes>                                                                    ← 无恢复(spill 失败回退)
<truncated: hit head_limit of N matches | recover: narrow the pattern or raise head_limit>            ← 总数未知的 B 变体
… <truncated: line over 500-char cap | recover: read_file <path> offset <line>>                      ← grep 行级(行内后缀)
```

生成走 `tool_output::truncation_marker` / `hit_limit_marker`(golden 测试钉
死);新增截断点必须复用,不得手写 format!。

## RULE-E-009:char-boundary 是硬规则

`head_tail_truncate` 是全库**唯一**实现,切片经 `floor/ceil_char_boundary`
落在 UTF-8 字符边界。历史教训(本任务修复):pre-C6 的 shell
`head_tail_preview`/`truncate_output` 与 `background_shell/in_memory.rs`
镜像副本裸切字节 —— >30KB CJK 输出在 1KB 预览处高概率 panic,
`tests_shell` 零 multibyte 基线所以从未暴露。「旧行为等价迁移」**不适用**
于 panic→安全截断的变化(PRD C5 豁免);新截断点必须带 multibyte 用例。

## spill 落点:`<app_data_dir>/outputs/<session_id>/`(不在项目树)

三依据:① agent 自我污染(spill 内容就是本 session 搜索的关键词,grep 会
命中自己的排泄物);② git 噪音(无 .gitignore 注入,untracked 暴露 +
`git add .` 误提交);③ 语义混杂(项目内 `.everlasting/` 是用户手写配置)。

**三件套捆绑,缺一即倒退**:

1. **落点**:`tool_output::spill(data_dir, session_id, bytes)` —— 字节入口
   (background_shell stdout/stderr 可能非 UTF-8,不做有损转换落盘);
   session_id 走 `execute_tool` 既有参数流(shell 的 parity 参数启用,
   registry 走 `new_with_data_dir` 注入;`None` 退 `_no_session` 目录)。
2. **权限 carve-out**:`sensitive.rs::build_trusted_external_patterns` 含
   `<app_data_dir>/outputs/**`(read 族免 ask,信任推理同 worktrees)。
   没有它,read_file 恢复 spill 每次弹窗,模式 A 名存实亡。
3. **sweep**:`delete_session` 调 `sweep_session_outputs`(session 整目录
   删,NotFound 静默)+ legacy `cleanup_outputs_dir(cwd)` best-effort(迁移
   前存量);background_shell 与 shell 共享同一路径,只迁一个会失配。

## 管道排空:>64KB 死锁教训(wait 前先 take 管道)

`child.wait()` 不读 stdout/stderr —— 输出超过管道容量(~64KB)时子进程
写阻塞、永不退出,烧满 120s 超时(pre-C6 存量缺陷:**spill 对 >64KB 输出
从未生效过**,CJK 测试暴露)。正解:select 前把管道 take 出来、spawned
task 并发排空(`spawn_pipe_drain`/`collect_drain`,shell 与
background_shell 共享一份);cancel/timeout 臂杀完进程组后从 task 收部分
输出。新增「跑子进程 + select 等待」的代码必须套这个形状。

## token 治理分层(与 pattern-budget-gate 的分工)

**截断 = 工具侧单发防御**(per-tool bytes,本模块);**关卡⑤ budget gate
= 轮级聚合防御**(统一预算表 + 0.95 硬卡,见
[pattern-budget-gate](./pattern-budget-gate.md))。两层各管各的,不互相
替代:截断保护的是单次 tool_result 的 context 占用,budget gate 兜的是
整个请求的总量窗口。

## 消费方速查(2026-08-30)

| 工具 | 截断点 | 恢复模式 |
|------|--------|---------|
| shell | >30KB spill(必经预览)+ 失败回退 50KB 头尾;cancel/timeout 臂同样过截断 | A / None |
| background_shell(in_memory) | >30KB spill + status 预览 1KB 头尾 | A / None |
| read_file | 50KB 头尾(offset/limit 切片后) | B |
| web_fetch | 5MiB body 硬错 TooLarge(不做流式);转换后 >100KB spill | A / None(测试路径) |
| grep | 每行 500 字符 + head_limit | B(行级 file:line 指引 + 升限提示) |
| glob | 100 条上限 | C(参照实现,保留原提示) |
