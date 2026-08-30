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

## P3 — 轻微(文档/一致性) [0 items]

> 全部 closed(RULE-PERM-001 于 2026-08-30 由 `.trellis/tasks/08-30-rule-perm-001-audit-pagination` 闭合:审计事件读 keyset `(ts,id)` 分页 + 过滤/计数下推 SQL,新命令 `list_session_audit_events_page` 双 transport additive 落地,旧全量命令零改动保留给 traceStore;dev DB 175 行真会话 live 冒烟 SMOKE OK;keyset 契约收编 spec `database-guidelines.md`,「过滤下推后全集派生值换口径」收编 spec `frontend/state-management.md`,详见 git log)。

> RULE-TEST-003 于 2026-08-30 由 `.trellis/tasks/08-30-e2e-red-fix` 闭合:两红均定性测试侧、生产零 diff —— ① e1a fixture 补 B1 后必填的 `supports_images`(422 = serde Json rejection);② e1b 整模块移除(4 个纯 SseRegistry 单元副本,WP4 改 compute_replay 语义时镜像漂移,权威副本在 sse.rs 内联 15 例)。`cargo test --test e2e` 同步进 CI Rust job,基线不再无人跑。spec daemon-server.md 收编「SSE 契约测试唯一 home + e2e 路由级定位 + 422 排查第一嫌疑」。详见 git log。

> RULE-TEST-001 于 2026-08-30 由 `.trellis/tasks/08-30-rule-test-001-browser-pipeline` 闭合:runner 定案 `@playwright/test` 进 `app/` devDependency(与 ui-review 的 scratch playwright-core 并存互不污染,共用 `~/.cache/ms-playwright` 按 build 版本隔离);环境 = vite dev :1422 + route-mock 全拦截(catch-all miss 500 fail-loud,堵 vite `/api` proxy 漏到真 daemon 的静默面)+ fake EventSource(无重连,重连语义归 Rust e2e);3 条试点 spec 7 用例覆盖键盘(CH5-1 Shift+Enter/Enter)/ 滚动联动(CH8-2 提问卡强制回底 + CH8-2b 排队并存 toast)/ 指针+弹窗(CH7-4 撤销确认 + backdrop click.self)三类 jsdom 盲区;CI frontend job 追加 cache + `install --with-deps chromium` + `test:e2e` blocking(D2:确定性准入,retries CI 1/本地 0);判据与 fixture 契约收编 spec `frontend/browser-regression.md`,本地运行补 HACKING-wsl。`app/src` 零 diff(仅消费既有 CH7-4 testid),e2e 7 + vitest 1466 + build 三绿。详见 git log。

> RULE-ALLOW-001 / RULE-SHIM-001 / RULE-HEALTH-001 / RULE-DOC-001 / RULE-DOC-002 / RULE-FE-002 / RULE-BUILD-001 于 2026-08-30 由 `.trellis/tasks/08-30-p3-debt-batch-cleanup` 闭合:三把模块级 dead_code 伞摘除(fallout 仅 1 项 —— `search_memories_fts` 定性为测试锁定件恢复 + 逐项 allow,`RecallStatusFilter::ActiveVerifiedOnly` 逐项 allow 注明策略旋钮;workflow/task.rs 与 memories 两伞零残留);`get_pending_question` + `test_provider` 两个 deprecated IPC 全链删除(Tauri 注册/daemon 路由/http 映射/常量/all_command_names/e2e 路由清单,messageTimeline 双渲染路径定性为旧 DB 行数据兼容并修正过时注释);health `session_count` -1 哨兵删除(零消费方,wire 删字段);drive.rs 72 处任务名注释收敛 + subagent mod.rs max_turns 20→200 修正;Yolo confirm/cancel 的 resolve 失败接 warn toast(+2 vitest);vite manualChunks 核实已实现(build 零告警)。详见 git log。

> RULE-SMOKE-001 / RULE-PERM-002 于 2026-08-27 由 `.trellis/tasks/08-27-rule-smoke-perm-cleanup` 闭合:turn-smoke.sh 的 send_and_wait 改等 SSE 请求终态(`chat-event` `kind=done`,每请求恰一次;常驻订阅 + trap 清理,--assert-turn-usage 复用同一日志),多轮工具 turn 不再被 delete_session 腰斩(live 双场景验证);grant 入口 `grant_tool_permission_inner` 按 `classify_tool` 校验 kind↔类别矩阵(Shell 只许 prefix 等,InvalidRequest 拒绝死数据组合);同任务顺带修复同族坑 —— AllowAlways 在 run_background_shell 上写的 prefix 行因 `check_prefix_grant` 硬编码 `tool_name='shell'` 永不命中,读侧放宽 `IN ('shell','run_background_shell')`。校验矩阵 + 终态 Done 不变量收编 spec `permission-layer.md` §4.3 与 `agent-loop-architecture/pattern-terminal-done-event.md`。详见 git log。

> RULE-TEST-002 于 2026-08-27 由 `.trellis/tasks/08-27-rule-test-002-role-gate-it` 闭合:新增集成用例 `role_gate_denies_then_allows_after_mid_loop_task_json_status_change`(`tests_agent_loop/role_gate_refresh.rs`)——round-1 denial(planning 拒 checker)→ mock LLM 同轮 write_file 翻盘 task.json status(in_progress,事故真实形态)→ round-2 经 drive_turn 轮顶刷新后同一 dispatch 放行(worker marker + call_count==4);变异验证覆盖两类回归(门误接入口快照 / R4 轮顶刷新移除),均精确转红后复原。已知边界:若 `resolve_current_task` 中途恒返 None 门会静默开放,该第三类未覆盖(spec tests-required 条目已注明)。全量 2008 后端测试 + fmt/clippy 绿,生产代码零 diff。详见 git log。

---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 git log) |
| P1 | 0 | 全部 closed(RULE-PERSIST-001 2026-08-24 闭合) |
| P2 | 0 | 全部 closed(RULE-QUEUE-001 2026-08-29 闭合,见上方 P2 段) |
| P3 | 0 | 全部 closed(RULE-PERM-001 2026-08-30 闭合,见上方 P3 段) |
| **Total** | **0** | 当前 open items |

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须从本文件删除已 closed 债项**(本文件 = open 集合,通过 `git log` 追溯 closed)
- **每条 finding 闭合后从本文件删除**(无 status 字段,文件存在即 open)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入
