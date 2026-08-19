# handoff 跨 session 接力 — 实施清单

依据 design.md;顺序按依赖排。每步完成即跑对应验证,最后全量。

> **进度(2026-08-19)**:①-⑥ 全部完成——后端 16 新测(handoff.rs)+
> route 冒烟;前端 9 新测(slashCommand +4 / MessageItem +6);⑤ 全量:
> 后端 1853 过(2 个 main 既有 subagent flaky,隔离复现通过,与 handoff
> 无关)、前端 1115 全绿、vue-tsc 0、clippy 新增 0(main 预存 1:chat_loop
> 8 参);spec 已回写 pattern-llm-compaction §handoff。⑥ live 冒烟
> PASS(daemon release 重编重启后):真实 LLM 接力 → child_title 继承
> "接力: …" / seq=1 kind=handoff_summary prefix 落库 / 双向 metadata
> children=1 / parent 行数不变(AC4)/ 子会话续跑一轮 7789 ctx tokens
> 正常;两会话自动清理。AC1-AC5 全勾。分层偏差见 design.md 头部记录。
> 环境注意:daemon 二进制设 PDEATHSIG=SIGTERM,在我这类沙箱 shell 里
> `daemon.sh bg` 的父 shell 退出即被杀——live 冒烟用常驻后台任务跑
> `daemon.sh start`(前台模式)绕开。

## 顺序清单

### ① 后端:校验 + 编排核心(compaction.rs)
- [ ] `validate_handoff_summary(text) -> Vec<HandoffMissingSection>`:Work State / Next Step **子串**匹配(模板第六段标题实为 "Optional Next Step"),标题在且正文非空
- [ ] `run_handoff(db, session_id, provider, context_window, focus, rows)`:prior 增量合并 → 全量 compressible(不切保留区;**prior 存在时复刻 anchor_msg 占位构造** `compressible = [anchor_msg(prior)] + candidates`,否则 builder 跳过 compressible[0] 静默丢 prior 后最旧一条,tokens_before 视图同镜像)→ 生成 → 校验(缺段二次重试带纠正块,恒缺报 `SummaryMissingSections` 不建 session)→ prior 快路径(无新 regular 行直接用 prior.content,缺段退化 LLM)→ 空会话 `NothingToHandoff`
- [ ] 熔断记账对齐 manual:最终成败记,中间重试不计
- 验证:`cargo test -p everlasting --lib "handoff"`(新 tests_agent_loop/handoff.rs:校验矩阵 / 重试 / 快路径 / 零副作用)+ 全量 `compaction` 50 测不回归

### ② 后端:落库层
- [ ] `insert_handoff_summary`(session_crud.rs,insert_compaction_summary 旁):content = 单 Text 块 JSON(prefix+摘要),text = 同内容纯文本,metadata kind=handoff_summary 全量字段,seq=1 游标契约
- [ ] session 创建继承(create 后 UPDATE 路线):db::create_session 传 project_id/current_cwd/model/model_id/**metadata(child.handoff 块创建即写入,零 RMW)**;后置 rename_session(接力标题 80 截断)/ set_worktree_state(三列一次)/ set_session_workflow_enabled / set_session_plugin_name / set_session_mode_internal(仅 parent.mode != edit);任一失败 best-effort delete_session 清空壳再报错
- [ ] parent 侧 metadata 读-改-写 helper(handoff_children 追加,不 clobber 已有键)
- 验证:`db/sessions_tests/handoff_summary.rs`:落库契约 / apply_compaction_watermark 忽略 handoff kind(wire 中为普通 user 行) / metadata 合并幂等 / 继承字段断言(七参 + 五 UPDATE)

### ③ 后端:命令层 + 注册
- [ ] `handoff_session_inner`(sessions.rs,compact_session_inner 旁):gate 链逐条镜像(群聊/llm_compaction_enabled/in-flight/provider 查找/错误映射)
- [ ] Tauri 命令注册 lib.rs;daemon 路由 POST /handoff_session(body {session_id, focus},与 compact 同构,routes/sessions.rs)
- [ ] resource_loader BUILTIN_COMMANDS 加 `handoff`
- 验证:命令层集成测(gate 拒绝矩阵)+ route 冒烟 + builtin 注册断言

### ④ 前端
- [ ] slashCommand.ts BUILTIN + matchBuiltinCommandInput;chat.types.ts `HandoffResult` / `HandoffSummaryMeta`
- [ ] ChatInput.vue executeBuiltin case handoff:toast → invoke → 刷新列表 + 切 new_session_id → 成功/失败 toast;palette focus 提取镜像 compact
- [ ] MessageItem:kind=handoff_summary 复用摘要卡片外壳,徽标"接力自 {parent_title}" + 点击跳 parent
- 验证:`cd app && pnpm test`(slashCommand/分发/卡片)+ `vue-tsc` 0

### ⑤ 全量回归 + spec 回写
- [ ] 后端全量:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 照 HACKING-wsl)+ `cargo test -p everlasting-remote`(不动 remote,跑一遍确认)
- [ ] clippy 0;前端全量 pnpm test
- [ ] spec:pattern-llm-compaction §handoff(行契约/全量覆盖语义/metadata 契约 + json_extract 审计样例);frontend chat spec handoff 卡片段

### ⑥ live 冒烟(AC5)
- [ ] turn-smoke.sh 加 `--handoff` 模式:临时 session 跑数轮 → handoff 路由 → 断言新 session/首行契约/续跑一轮/双向 metadata → 清理两 session
- [ ] 重编 daemon 后实跑 PASS,结果记 task notes

## 验证命令速查

```bash
# 后端(WSL 需 PKG_CONFIG_PATH)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib "handoff"
PKL=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig
cargo test -p everlasting --lib   # 全量(根目录跑,PKG_CONFIG_PATH=$PKL)
cd app && pnpm test               # 前端
# live
node scripts/remote-e2e-smoke.mjs          # remote 回归(如时间允许)
bash scripts/turn-smoke.sh --handoff       # 新增模式
```

## 风险文件 / 回滚点

- `app/src-tauri/src/agent/compaction.rs`——共享文件,只新增不改旧函数;回归锚:全量 compaction 测试
- `app/src-tauri/src/commands/sessions.rs`——大文件,只加不改;回归锚:manual_compaction 集成测
- 回滚策略:纯新增为主,任一步失败可单独 revert 对应 commit,不牵连 /compact 与自动压缩路径

## start 前检查

- [ ] prd.md 收敛版无 Open Questions 残留
- [ ] user 对最终规划摘要明确批准
- [ ] `python3 ./.trellis/scripts/task.py start`(inline 工作流,implement.jsonl/check.jsonl 门豁免——Phase 2 走 trellis-before-dev)
