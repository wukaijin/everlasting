# Review Backlog — 跨 review 债务整合

> ## ⚠️ 强制约定(2026-06-23 强化)
>
> 本文件**只记录当前 open 技术债**。
>
> - finding 解决后必须**从本文件删除**(通过 `git log` 追溯,不在此保留 closed 条目)
> - **严禁**记录任何日志 / 流水 / 决策历史 / 降级说明 / 收尾路径建议 / 子 task 编排 / Feature Follow-ups / Re-evaluation Log / 历史最后更新
> - 上述内容走 journal 或独立 spec 文档,**不允许污染本文件**

---

> **目的**: 集中追踪所有 review(审计 / SPEC 对照 / 历史 review)的 finding,避免下次 audit 重新独立复述
>
> **基线审计**: `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`(commit `a4fb302`)
>
> **创建**: 2026-06-14(由 `.trellis/tasks/06-14-review-debt-consolidation` 启动)

---

## 新增 finding 流程

> **重要**: 任何新 audit / review / spec 对照,**第一步必须 diff 本文件**。

### 添加新 finding

```markdown
### RULE-{Subsystem}-{Seq}

- **Level**: P0 | P1 | P2 | P3
- **Subsystem**: Agent Loop | Permission | Memory | Provider | Tools | Cross
- **File**: `path/to/file.rs:LINE`
- **Description**: 一句话描述问题
- **Fix**: 修复方向(行数估算)
- **Owner**: carlos | 待分配
- **Related Task**: `.trellis/tasks/XX-YY-name` 或 null
- **Discovered In**: `docs/_reviews/REVIEW-XXX.md`
```

### 流程规则

1. **不重新展开已记录 finding**: 新 audit 中遇到已记录的 RULE-X-XXX,**只标一行** `// See DEBT.md §RULE-X-XXX`,不重新描述 file:line 和影响
2. **闭合时**: PR merge 后从本文件删除该 finding;通过 `git log` 追溯;**本文件 = open 集合**
3. **优先级重审**: 每次 audit 可重新评估,如需降级或合并,在 finding 描述中注明理由
4. **ID 一旦分配不变**: 即使 finding 后续证明不是问题,ID 不重新分配(已删除的 ID 可留空)

### 复述检测

如果新 audit 复述了某条 finding 但未引用 DEBT.md:
- **轻度**: review 本身不扣分,但应在结论段标注"漏查 DEBT.md"
- **重度**: 如果是 P0/P1 漏查,review 应被打回修订

---

> **本文件仅记录当前 open 债项**。已 closed 条目不在此保留;通过 git log 追溯。

## P1 — 重要(正确性 + 资源) [0 items]

> 全部 closed(RULE-PERSIST-001 于 2026-08-24 由 `.trellis/tasks/08-24-p1-turn-crash-recovery` 闭合,详见 git log)。

## P2 — 健壮性 + 债务,中长期清理 [0 items]

> 全部 closed(RULE-QUEUE-001 于 2026-08-29 由 `.trellis/tasks/08-29-rule-queue-001-multi-drain-persist` 闭合:init.rs 非尾条 persist 循环 + `ChatLoopRequest.drained` 全量载体;多 drain 全落库集成测试含全 manual 对照;spec driver-pattern / scheduled-tasks origin 链 / signature 三处同步收口,详见 git log)。

> RULE-CI-001 / RULE-FM-001 / RULE-TESTPOOL-001 于 2026-08-26 由 `.trellis/tasks/08-26-p2-debt-cleanup` 闭合(clippy gate 落地 / frontmatter 解析收敛 `parse_md_resource` + `parse_string_array` / `db/test_support.rs` 共享 `test_pool()` 15 处替换),详见 git log。

> RULE-ARGS-001 于 2026-08-27 由 `.trellis/tasks/08-27-rule-args-001-param-object` 闭合:parameter object 落地(`run_chat_loop` 38→3 参,drive_turn 49→6、dispatch_tool_calls 33→4;ChatLoopDeps/ChatLoopRequest/CallerRole 三套件统一经 `ChatLoopDeps::from_app_state` 构造),chat_loop 家族 `too_many_arguments` 豁免归零(全库 46→34);全量回归 + clippy gate 验证通过;spec `signature-run-chat-loop.md` 同步重写为三套件契约。详见 git log。

> RULE-SHELL-001 于 2026-08-27 由 `.trellis/tasks/08-27-rule-shell-001-sweeper` 闭合:daemon 侧 `spawn_shell_sweeper` 落地(5min interval 调 `sweep_completed_shells(SHELL_RETENTION_MS=1h)`,只清 Done 超龄条目,释放 stdout/stderr 缓冲;Running/通知队列/spill 文件不动;GUI 路径零改动);装配契约收编 spec `daemon-server.md` §运维伴生物。详见 git log。

> RULE-FE-001 于 2026-08-27 由 `.trellis/tasks/08-27-rule-fe-001-objecturl-revoke` 闭合:send 成功释放 staging strip 时逐 `uploaded[].localUrl` revoke(镜像 `discardStagedImages` 先例,~3 行);债条登记的"reloadAfterFinalize 替换钩子"方向前提已证伪——渲染层 `MessageImages.urlFor` 自 B1 PR5 起 file 优先(daemon GET 路由),blob URL 是从不触发的防御回退,故无需动 reload 枢纽;B1 strip 生命周期(objectURL 三路 revoke + jsdom spy 测试 gotcha)收编 spec `state-management.md` §Chat Store Action Clusters。上传失败不 revoke 保条、F1 排队 / cancel / interrupted 各路径零回归(1268 前端测试 + vue-tsc 全绿)。详见 git log。

## P3 — 轻微(文档/一致性) [9 items]

> RULE-SMOKE-001 / RULE-PERM-002 于 2026-08-27 由 `.trellis/tasks/08-27-rule-smoke-perm-cleanup` 闭合:turn-smoke.sh 的 send_and_wait 改等 SSE 请求终态(`chat-event` `kind=done`,每请求恰一次;常驻订阅 + trap 清理,--assert-turn-usage 复用同一日志),多轮工具 turn 不再被 delete_session 腰斩(live 双场景验证);grant 入口 `grant_tool_permission_inner` 按 `classify_tool` 校验 kind↔类别矩阵(Shell 只许 prefix 等,InvalidRequest 拒绝死数据组合);同任务顺带修复同族坑 —— AllowAlways 在 run_background_shell 上写的 prefix 行因 `check_prefix_grant` 硬编码 `tool_name='shell'` 永不命中,读侧放宽 `IN ('shell','run_background_shell')`。校验矩阵 + 终态 Done 不变量收编 spec `permission-layer.md` §4.3 与 `agent-loop-architecture/pattern-terminal-done-event.md`。详见 git log。

> RULE-TEST-002 于 2026-08-27 由 `.trellis/tasks/08-27-rule-test-002-role-gate-it` 闭合:新增集成用例 `role_gate_denies_then_allows_after_mid_loop_task_json_status_change`(`tests_agent_loop/role_gate_refresh.rs`)——round-1 denial(planning 拒 checker)→ mock LLM 同轮 write_file 翻盘 task.json status(in_progress,事故真实形态)→ round-2 经 drive_turn 轮顶刷新后同一 dispatch 放行(worker marker + call_count==4);变异验证覆盖两类回归(门误接入口快照 / R4 轮顶刷新移除),均精确转红后复原。已知边界:若 `resolve_current_task` 中途恒返 None 门会静默开放,该第三类未覆盖(spec tests-required 条目已注明)。全量 2008 后端测试 + fmt/clippy 绿,生产代码零 diff。详见 git log。

### RULE-ALLOW-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `app/src-tauri/src/db/memories.rs:14-42`(模块级 `#![allow(dead_code)]`,注释自承诺"P2 落地后替换为逐项 allow")、`agent/workflow/{def,state,task}.rs`(三个模块级大伞待审计)、subagent transcript/sink/event_sink 观测预留面
- **Description**: 模块级 dead_code 大伞掩盖 typo/refactor 孤儿函数,memory P2–P5 已全部上线但收窄承诺未兑现;对照组 auto_reflect.rs 同类 allow 已按承诺拆除
- **Fix**: memories 模块逐项收窄(约半天)+ workflow/subagent 预留面逐个审计后拆除或登记真实用途
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-SHIM-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `commands/question.rs:529`(`get_pending_question` 标 deprecated,lib.rs:423 注册 + `#[allow(deprecated)]`)、`commands/providers.rs:367`(`test_provider` 标 DEPRECATED,前端已走 `test_model`)、`components/chat/messageTimeline.ts:18`(contentBlocks ↔ 旧三桶数组双渲染路径)
- **Description**: 弃用兼容面长期并存且无下线时间表,靠压制属性维持编译静默;风险是新旧路径行为漂移无人察觉
- **Fix**: 确认实际调用方归零后从注册表删除,或写明下线节点(合计约半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-PERM-001

- **Level**: P3
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/commands/permissions.rs:367-371`
- **Description**: 审计事件查询 MVP 全量拉取,无分页无虚拟滚动(PRD Edge Cases 标 TODO ">500 条事件的 session");索引让 ORDER BY 够快故暂无实测投诉
- **Fix**: LIMIT/OFFSET 或 keyset 分页(1–2 天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-HEALTH-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `app/src-tauri/src/daemon/routes/health.rs:102`(TODO(P2.5))
- **Description**: health 接口以 `-1` 哨兵上报 session_count;带 AppState 的完整变体与 `/api/v1/health/detailed` 显式推迟 P2.5 至今未动(Q1 需要 stateless router 先应答端口探测)
- **Fix**: 接线 AppState 或删哨兵语义,动手前先确认 sidecar 握手是否有消费方(半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-DOC-002

- **Level**: P3
- **Subsystem**: Cross
- **File**: `agent/subagent/mod.rs:28`(注释 `max_turns: Some(20)` vs 实际 `SUBAGENT_MAX_TURNS`=200,journal-3:18 known drift)、`.trellis/spec/frontend/state-management.md:517`(仍在描述已迁入 MessageItemEdit.vue 的 MessageItem 内联编辑)、`app/src-tauri/TECH.md` §1.4(serde_yaml 段与 frontmatter 手写解析现状不符)
- **Description**: 实现↔文档三处漂移,均已发现并躺 journal 但未销账;journal 不是 open 债台账故收编于此
- **Fix**: 三处各一小段修正(合计约半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(journal 未销账项收编)

### RULE-FE-002

- **Level**: P3
- **Subsystem**: Frontend
- **File**: `app/src/stores/chatModeActions.ts:225-232`
- **Description**: Yolo 确认后 `resolve_mode_change` 失败只写 console.error,不进 toast/pending 卡片,用户无从得知模式切换失败(follow-up 注释指明可复用现有 pending-card re-mount 路径)
- **Fix**: 失败分支接入现有提示通道(约半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-BUILD-001

- **Level**: P3
- **Subsystem**: Frontend
- **File**: `app/vite.config.ts`(产物超限 chunk 警告)
- **Description**: 前端构建存在超限 chunk,vite 每次构建输出告警;manualChunks 拆分方案已在 journal 定向但被标"单独任务"搁置(journal-3:283)
- **Fix**: manualChunks 按 vendor/UI 分包 + 构建体积复核(约半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(journal 未销账项收编)

### RULE-TEST-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `agent/tests_request_mode_change.rs:32`(`resolve_mode_change` IPC 的 DB 更新链路零覆盖,"TODO: future task")、`components/common/MarkdownDetailModal.test.ts:383-390`(jsdom 无法模拟 pointerdown-outside,仅占位守护)
- **Description**: 两处测试基建缺口:mode change 持久化链路零集成覆盖(目前靠共享纯函数既有测试垫底);项目无真浏览器 runner,跨组件指针交互类回归无可靠守护手段
- **Fix**: 该链路补 1 条集成用例 + playwright 选型评估
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-27 技术债盘点(AI 全库扫描)

### RULE-DOC-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `app/src-tauri/src/agent/chat_loop/drive.rs:107`(参数注释块 `08-20-turn-usage-event-quota-view WP2` 等)、`docs/CONTEXT.md`(与 CLAUDE.md "当前状态" 段重复)
- **Description**: 参数注释块把 git log 已记录的 feature 名 + 日期 + commit hash 重复进代码注释,形成双 source of truth(注释会被 feature 重命名牵动,gir log 是只读稳定副本);CLAUDE.md "当前状态"段与 ROADMAP 重复且每轮注入付 token 税
- **Fix**: 参数注释收敛为一句用途说明,历史走 git log;CLAUDE.md 状态段改派生生成(git log / 代码现状 / 既有 memory 管道)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 群聊 session `702e6ec8…`(讨论本项目不足,代码事实验证)


---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 git log) |
| P1 | 0 | 全部 closed(RULE-PERSIST-001 2026-08-24 闭合) |
| P2 | 1 | RULE-QUEUE-001(F1-A 多 drain 非尾条不落库,F2 planning review 2026-08-28 登记) |
| P3 | 10 | 文档 + 一致性 + 待兑现承诺,可延后 |
| **Total** | **11** | 当前 open items |

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须从本文件删除已 closed 债项**(本文件 = open 集合,通过 `git log` 追溯 closed)
- **每条 finding 闭合后从本文件删除**(无 status 字段,文件存在即 open)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入
