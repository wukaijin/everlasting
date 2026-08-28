# Design — LLM schedule_task tool(detached dispatch)

> PRD:`prd.md`(2026-08-29 收敛)。本文件记录技术定案;实施序见 `implement.md`。

## 1. 架构与边界

```
LLM turn ── tool_use ──▶ execute_tool_inner(match name)      ← plain dispatch,无 chat_loop 拦截
                            │
                            ├─ schedule_task    ──┐
                            ├─ schedule_status  ──┼─▶ tools/scheduled_task_family.rs(新模块,三件)
                            └─ schedule_cancel  ──┘         │
                                                             ▼
                                    commands/scheduled_tasks.rs::create_scheduled_task_inner
                                    (Q0 单源,校验矩阵全复用;新增 created_by 参数)
                                                             │
                                                             ▼
                                    db/scheduled_tasks.rs(insert 参数化 created_by;
                                    list 增 created_by 过滤)
                                                             │
                                    到期 fire = 既有 scheduler_tick(零改动)
```

边界承诺:**调度判定 / 队列 / origin 载体链 / fire 审计零改动**(spec `scheduled-tasks.md`
契约不动);本任务全部改动在「创建/查询/删除的作者入口」一层。

## 2. 关键定案

### D1 三工具拆分(单模块)

命名家族镜像 L1a 三件套(`run_background_shell`/`shell_status`/`shell_kill`):

| tool | 动作 | 关键参数 |
|---|---|---|
| `schedule_task` | create | `name` / `prompt` / `schedule`(6 档 JSON)/ `max_runs?` / `ends_at?` / `in_current_session?` (default false) |
| `schedule_status` | list(本项目 agent 任务) | 无必填 |
| `schedule_cancel` | 硬删 | `task_id` |

- 不做单工具多 action:三个 schema 各自小而明确,LLM 可靠性优先;token 成本由 R8
  stub 化吸收(STUB 后 `tools[]` 增量 ≈ 每具一行 stub)。
- 三具同置一个新模块 `tools/scheduled_task_family.rs`(共享校验/错误文案/常量;
  先例:`tools/merge_worker.rs` + `merge_worker/` 子模块的组织方式)。
- 注册:**`builtin_tools()` 尾部追加**,顺序 = schedule_task → schedule_status →
  schedule_cancel(追加不扰动既有前缀;家族相邻便于 stub 断言)。

### D2 created_by 参数化(db 层)

- `NewScheduledTask` 增 `created_by: String` 字段;`insert_scheduled_task` SQL 的
  字面 `'user'` 改绑定参数(db:108 注释预告的做法)。既有两个调用点
  (`create_scheduled_task_inner`、db 单测)显式传 `'user'`,行为零变化。
- `list_scheduled_tasks` 增 `created_by: Option<&str>` 参数(`None` = 不过滤,既有
  调用点传 `None`);`schedule_status` 传 `Some("agent")` + 当前 project。
- 取消/删除所有权防御:`schedule_cancel` 执行侧先 `get_scheduled_task` 校验
  `created_by == "agent"`(不匹配 → 结构化错误,不触碰行);随后走既有
  `delete_scheduled_task`(幂等)。

### D3 上限 gate(R4)落点:tool 侧,不进 `_inner`

- `create_scheduled_task_inner` **不加**上限/kill switch 检查 —— 用户 UI/IPC 路径行为
  必须零变化(AC6 后半句)。两个 gate 都在 tool execute 里、调 `_inner` 之前:
  - kill switch:`db::config::get_config_value(ctx.db, "scheduled_tasks_enabled")`
    == 字面 `"false"` → 拒(镜像 `scheduler/mod.rs:229-232` 的 fail-open 判定,同一
    字符串比较,抽出共享常量避免两处字面漂移)。
  - 上限:`COUNT(*) WHERE project_id=? AND enabled=1 AND created_by='agent'` ≥ 20
    (常量 `MAX_ACTIVE_AGENT_TASKS: i64 = 20`)→ 拒,错误信息含
    「先用 schedule_cancel 取消不用的任务」自愈指引。COUNT → INSERT 两步存在 TOCTOU
    窗口(同 project 两个并发 agent turn 可瞬时 21+):**有意接受** —— 单用户桌面 +
    反滥用软上限语义,不做原子化;cap 测试注释写明,防未来被当 bug 误修(评审 P2-2)。
- 顺序:kill switch → 上限 → `_inner` 校验(便宜的先抛)。

### D4 权限与审计(R6)

- `classify_tool` **不加 arm**:三具自然落 `ToolKind::Other` = Tier 5 silent Allow;
  `risk_for_tool` 走 `Risk::Low` 缺省;Plan mode 不剥(写 DB 不写盘,同 `remember`)。
- 审计:Tier 5 分支自动记 `ToolAllowed`(tool 名 + input);无需新 AuditKind。
- 并行白名单:三具不加入 `is_parallel_eligible`(写性/IO 混合,缺省 serial,零改动)。

### D5 worker / 群聊隔离(R3)

- `STRUCTURALLY_DISABLED`(`agent/subagent/tools_filter.rs:24`)追加三名(worker 不得排
  detached 任务,镜像 `create_task` 注释理由:编排面归 parent)。
- 群聊隔离**零改动**:`group_chat_tool_defs`(`agent/group_chat_prompts.rs:216`)是
  穷举白名单(`GROUP_CHAT_RESEARCH_TOOLS` + moderator 两件),新注册的 builtin 工具
  不会自动进入群聊 —— 这是 08-07 从黑名单改白名单的教训买来的性质。补 AC4 断言测试
  锁定(moderator / participant 两态 toolset 均无三具)。
- **禁止**把三具加进 `filter_tools_for_session_type` 剥除名单 —— 该名单语义是「从
  普通 chat 剥除群聊专属工具」(群聊分支 early-return 全量保留),加进去会把三具从
  普通 chat 剥掉、群聊反而可见,恰好反向(初稿设计错误,评审 P1 修正);其模块注释
  也明令不得越俎代庖 second-guess 群聊白名单。
- 执行侧防御:不做(与 L1a 三件套同宽 —— filter 是唯一防线是既有实践;cancel 的
  created_by 校验(D2)已挡住跨作者破坏;群聊 target 由 `_inner` 的
  `validate_target_session` 作纵深防御)。
- `READONLY_TOOL_ALLOWLIST` 不加(三具非只读)→ 并发只读 worker 天然不可见。

### D6 target session(R7)

- `in_current_session=true` → tool 以 execute 参数拿到的 `session_id` 作
  `target_session_id` 传入 `_inner`(既有校验自动过:当前 session 必 chat、必属当前
  project;群聊已被 D5 剥除,双保险)。`session_id` 为 `None`(理论不可达)→ 结构化
  错误。
- 缺省(新专用 session)直接走 `_inner` 的 None 分支(新建 + 标题=任务名),零新代码。

### D7 schema 与提示词

- `schedule` 参数:传 **6 档 JSON 对象**(非字符串),description 逐档一行枚举形状
  (daily/interval/weekly/hourly/weekdays/monthly),与 `parse_schedule` 接受的形状
  一致;tool 侧 `serde_json::to_string` 后传 `_inner`(其入参是 String)。
- `ends_at`:LLM 视角用「ISO 日期字符串 `YYYY-MM-DD`」比 epoch ms 可靠 —— tool 侧
  解析为当日 23:59:59.999 本地 epoch ms(F2b 语义,`_inner` 校验 >now)后传入;
  解析失败结构化报错。`max_runs` 透传(integer ≥1)。
- 错误文案全部中文(与 `_inner` 校验矩阵一致),JSON tool_result。

### D8 C7D stub(R8)

- `STUB_CANDIDATES`(`tools/stub.rs:34`)追加三名,`STUB_DESCRIPTIONS`(:62,定长
  `; 11`)同步 11 → 14(两处定长字面量,漏一即编译红)。
- 预算线平移:`static_token_budget_classic_chat_first_turn`(`tools/stub.rs:330`,
  线 3960 / 实测 ~3903)按校准注 3 既有政策「新注册工具逼近线 → 优先扩
  STUB_CANDIDATES,线随新增基线平移」处理 —— 三具 stub 化预计 +~100 tok,实施时
  实测后平移(≈ 3960 → ~4060,以实测为准)并补校准注 5。不改此步则步骤 4 提交即红,
  会被误判为回归。
- 不变量自动满足:∩ 并行白名单 = ∅(D4);worker/群聊已被 stub 适用 gate + D5 双重排除。

### D9 前端(可见性补偿,R2 尾)

- `ScheduledTaskPayload` 增 `created_by: String`(From 行补映射,additive wire)。
- Settings 定时任务 tab 行加来源小徽标:`agent` → 「agent」chip,`user` 不显示
  (缺省态零噪音)。样式走既有 badge 家族。

## 3. 兼容与迁移

- 零 DB migration(created_by 列已存在;本轮只是开始写 `'agent'` 值)。
- wire additive:payload 增字段、`tools[]` 尾部追加 —— 前端旧构建 / 旧 daemon 混跑
  均兼容(与 F6 busy additive 同款判例)。
- 既有测试零改动预期:insert 的两个显式 `'user'` 调用点之外,db 单测
  `created_by=='user'` 断言(`db/scheduled_tasks.rs:423`)继续成立。

## 4. 权衡记录

- **拆三工具 vs 单工具**:选拆 —— LLM 可靠性 + 家族先例;代价是 STUB_CANDIDATES +3、
  注册行 +3,被 stub 机制抵消。
- **上限 20 vs 不设**:选设 —— Q2 silent Allow 的补偿控制是定案前提;20 取「单项目
  自动化 routine 绰绰有余」量级,常量单点可调。
- **kill switch 只拦 tool vs 连 UI 一起拦**:选只拦 tool —— 改 UI 路径是 scope 蔓延,
  且用户显式创建时理应自见开关状态。
- **ends_at 收 ISO 字符串 vs epoch ms**:选 ISO —— LLM 对墙上钟可靠、对 epoch 易错;
  转换成本在 tool 侧一层。

## 5. 回滚

单 commit 粒度按 implement.md 步骤切;任一步出问题 revert 即回退,无数据迁移需要
回滚(created_by='agent' 行 revert 后仅成为「普通任务」,fire/编辑不受影响 ——
id 字段无语义耦合)。
