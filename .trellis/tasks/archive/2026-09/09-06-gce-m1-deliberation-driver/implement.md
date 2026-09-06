# Implement — GCE-M1 群聊审议驱动

## 顺序清单

1. **骨架 + 内省**:`scripts/group-chat-run.mjs`——arg 解析、daemon base URL(默认 `http://127.0.0.1:7456`,对齐 turn-smoke)、`projects` / `models` / `presets` 三子命令。
2. **run 前半**:resolve-project(按路径匹配 → miss 自动 create_project)、预设常量(review/arch/retro)+ `--participants`/覆盖合并(三级:增删/单人 model/单人 persona)、`--dry-run` 打印请求体。
3. **run 后半**:`create_session`(metadata participants + session_type=group_chat + model=moderator)→ 异步 `agent/chat`(首条 user wire = topic)→ 轮询 loop(10s,进度行 / `--quiet`)→ 终态分发。
4. **中断与退出码**:SIGINT/`--timeout`(默认 30min)→ `cancel_chat` → 等 cancelled 落定(≤60s 兜底)→ 部分转录;`--cleanup` 成功路径 `delete_session`;退出码 0/2/3/4,1 = 脚本自身错误。
5. **转录导出**:`load_session` → 渲染 `out/group-chat-<slug>-<ts>.md`(格式对齐既有三份:头 metadata + summary + seqN speaker 正文);`--out` 覆盖。
6. **skill**:`.agents/skills/group-chat/SKILL.md`(慎用警告/议题与 persona 写法/配方选择/结果解读/内省速查;零逻辑零漂移事实)。
7. **文档接线**:DAEMON-API.md 加一节(脚本即文档,链接 skill);AGENTS.md 冒烟速查区补一行。

## 验证命令

```bash
node scripts/group-chat-run.mjs projects          # daemon 在线:真实目录
node scripts/group-chat-run.mjs models
node scripts/group-chat-run.mjs presets
node scripts/group-chat-run.mjs run --preset review --topic "..." --dry-run   # AC4 参数级冒烟
node scripts/group-chat-run.mjs run --preset review --topic "..." --timeout 60 # AC2 中断臂(60s 超时触发 cancel)
node scripts/group-chat-run.mjs run --preset review --topic "<真议题>"          # AC1 live 全程
```

AC5(最终验收,用户定法):daemon 里对一个非 everlasting 项目开单聊,只给一句指引(`node <abs>/scripts/group-chat-run.mjs ...` +「召集一场关于 X 的审议」),该 LLM 自行内省组装、后台 shell 起 run、轮询拿结论。前置自测臂:本仓库 ZCode 会话仅凭 skill 完成一场。
另需 live 验证:外层 session busy 与内层群聊编排并发同跑(daemon 多 session 并发既有设计,首次实测此形状)。

## 风险点与回滚

- **`agent/chat` 确切 body 形状**:turn-smoke 先例是 `{request_id, session_id, messages}`;群聊首条 wire 是否有额外要求(如 mode 字段)实现时对照 `commands/mod.rs` 的 chat 入参与两场 live 的 DB 实迹核对,不确定就先 dry-run 对照。
- **cancel_chat 参数签名**:从 `commands/cancel.rs:36-89` 确认(session_id 还是 request_id)。
- **models 内省端点**:providers 域的具体命令名实现时确认。
- **外层 agent 能否自主后台化 shell**(AC5):确认 in-app shell 工具有 LLM 可用的后台模式(session 53 加的是可观测性;若无自主后台旗标,AC5 指引里教外层用 `&` + 输出重定向,或提前批一次 shell 权限)。
- **转录落点**:out/ 必须按脚本位置解析(design 已钉死),嵌套消费不散落;结束时 stdout 打印绝对路径。
- **live 验收成本**:一场数十万 token;AC1/AC4 各跑一场为宜但可合并(一场同时验 AC1+AC4-live),AC2 用 60s 短超时低成本触发。
- 回滚 = 删 `scripts/group-chat-run.mjs` + `.agents/skills/group-chat/` + revert 两处文档;零 daemon 改动保证无级联。

## task.py start 前检查

- [ ] inline 工作流,无需 implement.jsonl/check.jsonl 策展(Phase 2 经 trellis-before-dev 载入上下文)。
- [ ] prd.md 收敛版无遗留 Open Questions(已全部决入 Decisions)。
- [ ] design.md 的 M2 复用约束(纯函数区 + CLI 薄壳)在实现时遵守。
