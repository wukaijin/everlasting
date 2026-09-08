# implement.md — gce-m4c 执行清单

> 提交切分原则:每步独立可提交、可验证、可 revert;顺序 = 后端地基 → 三通道 → 弹窗 → 文档。R 侧标注对应 PRD 需求。

## Step 1 — db 核算查询 + 命令面(R3,daemon Rust)

- [ ] `db/trace.rs`:`GroupChatTokenUsage`/`SpeakerTokens` + `group_chat_token_usage(pool, session_id)`(SUM 四计费字段,GROUP BY speaker;join 约束照抄 `list_speaker_cache_usage`:assistant / speaker 非空 / usage 非空 / `run_id=''`)
- [ ] `db/usage_tests.rs`(或 trace 测试文件)聚合单测:多 speaker 多 turn 造数、四字段口径、worker 行排除、无 usage 行排除、retry 覆盖不双计
- [ ] `commands/sessions.rs` inner + Tauri command;`daemon/routes/sessions.rs` route(`POST /api/v1/sessions/group_chat_token_usage`);`lib.rs` invoke_handler 注册
- [ ] 验证:`cargo test -p everlasting --lib`(带 PKG_CONFIG_PATH)

## Step 2 — 讨论库 hit 加 total_tokens(R3)

- [ ] `db/search_group_chat.rs`:list/search 两查询加 correlated subquery SUM(同口径);`GroupChatSessionHit.total_tokens: Option<u64>`
- [ ] 单测:有消耗场返回数字、零消耗/无 trace 场返回 NULL
- [ ] `DiscussionLibraryModal.vue` 列表加消耗展示(万单位格式化,无数据「—」)+ vitest
- [ ] 验证:cargo --lib + `cd app && pnpm test`

## Step 3 — M4a 定时透传(R1)

- [ ] `db/scheduled_tasks.rs`:`GroupChatTaskConfig` + `token_budget: Option<u64>`(serde default)+ parse 校验(正整数)
- [ ] `scheduler/mod.rs` fire:metadata 改 Map 构造,Some 时插键(不写 null);既有 fire 测试补断言(带预算任务 fire → metadata 带键;不带 → 无键)
- [ ] TS `stores/scheduledTasks.ts` 类型 + `ScheduledTasksTab.vue` 第四档预算输入(留空 = 不限;编辑态预算 dirty 独立提交,见 design §2.3)
- [ ] **AC4 剧本**:MockProvider 单测——fire 建群(带小预算)→ 编排轮头硬停 `stop_reason=budget`(证明透传生效,非只写键)
- [ ] 验证:cargo --lib + pnpm test

## Step 4 — M1 script + MCP 透传(R1)

- [ ] `group-chat-run.mjs`:`--token-budget` flag(parse 校验)、`buildCreateBody` 增参、`--dry-run` 模板可见;`aggregateTokens(turnTraces, messages)` 导出 + 转录统计段带总消耗(失败降级)
- [ ] `group-chat-mcp.mjs`:shape 加 token_budget;coreStart 透传;coreResult stats 加 `tokens{total, per_speaker}`(复用 aggregateTokens;失败省略)
- [ ] 单测:`group-chat-run.test.mjs`(buildCreateBody 两态 + aggregateTokens 口径)+ `group-chat-mcp.test.mjs`(shape/透传/AC4 wire 锁重测——实测仍 <3200 则锁不动,超则升并在注释记实测)
- [ ] 冒烟:`node scripts/group-chat-mcp-smoke.mjs`(非 live)+ `group-chat-run.mjs --dry-run`
- [ ] 验证:`node --test scripts/group-chat-run.test.mjs scripts/group-chat-mcp.test.mjs`

## Step 5 — 共享逻辑提取(R2 前置)

- [ ] `app/src/utils/groupChatPresets.ts`:preset JSON 类型 + `resolveModelRef` + `composePersonaMd`(原 gcPersonaMd)+ GC_PRESETS 加载;ScheduledTasksTab 改 import(纯搬家,vitest 既有用例守护零行为变化)
- [ ] 验证:pnpm test(ScheduledTasksTab 相关用例全绿)

## Step 6 — 弹窗重设计(R2,最大前端件)

- [ ] create 模式:preset 单选卡区(选中预填,含主持人默认)+ 阵容微调(交互保留)+ 主持人 Select(preset 默认可改;提交传 modelId)+ 预算输入 + 量级提示文案
- [ ] edit 模式:成本区(per-speaker「N万 · 缓存 x%」合并行 + 预算进度条;`group_chat_token_usage` + `group_chat_cache_rates` 两查询,失败降级「—」)
- [ ] `chatSessionActions.createNewSession` 加 modelId 透传(`create_session` model 参数)
- [ ] vitest:GroupChatConfigModal.test.ts 重构扩展(preset 预填同形断言/persona 组装/主持人参数/预算两态/成本区渲染/上限交互不回归);ChatPanel 传参适配
- [ ] 移动端:@media 全屏块下 preset 卡/成本区布局核对
- [ ] 验证:pnpm test + vue-tsc + `pnpm build`;`scripts/ui-review.sh --screenshots-only` 过一眼(AC6)

## Step 7 — 文档接线(R4)

- [ ] DAEMON-API.md:§6.1 token_budget 段补三通道;§6.3 config 键;§3 讨论端点 total_tokens;核算端点新节
- [ ] GROUP-CHAT-API-ROADMAP.md §5:成本治理 ✅ 落账(M4 仅余远程暴露认证);总览表 M4 状态更新
- [ ] AGENTS.md:群聊速查行(--token-budget / 核算端点);`.trellis/spec/scripts/` 若 wire 锁升值则补记
- [ ] 检查 `docs/ROADMAP.md` 群聊第四档表述是否需同步

## 验证命令速查

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
cd app && pnpm test && pnpm exec vue-tsc --noEmit && pnpm build
node --test scripts/group-chat-run.test.mjs scripts/group-chat-mcp.test.mjs
node scripts/group-chat-mcp-smoke.mjs
scripts/ui-review.sh --screenshots-only
```

## 风险点 / 回滚

- **scheduler fire 测试**:fire 链路测试环境重(需要 pool + 编排);AC4 剧本若集成测试代价过高,退化为「fire 构造的 metadata 单元断言 + 既有 09-08 预算硬停测试(已证 metadata 键生效)」两级拼合,不硬造重集成。
- **wire 锁**:AC4 重测若超 3200,升锁需同步 smoke 断言与 spec 记录(锁值出现三处)。
- **ScheduledTasksTab 搬家回归**:Step 5 是 Step 6 的前置但独立提交;若搬家引出既有用例失败,回滚该步只影响弹窗新增(弹窗内联同逻辑临时兜底,不让任务卡死)。
- **WAL**:所有 DB 断言用 `sqlite3 -readonly`;直写造数先 `./scripts/daemon.sh stop`。

## 提交切分(预期)

1. `feat(db+api): group_chat_token_usage 聚合查询 + 命令面`(Step 1)
2. `feat(search): 讨论库 hit 加 total_tokens + GUI 列`(Step 2)
3. `feat(scheduler): 定时审议 token_budget 透传 + 预算生效剧本`(Step 3)
4. `feat(scripts): M1 --token-budget + MCP 参数与 result 核算`(Step 4)
5. `refactor(frontend): preset 共享逻辑提取`(Step 5)
6. `feat(gui): 建群弹窗重设计——preset 优先 + 主持人选择 + 成本区`(Step 6)
7. `docs: 成本治理落账接线`(Step 7)
