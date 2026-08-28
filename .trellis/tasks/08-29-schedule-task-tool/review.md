# Review — LLM schedule_task tool(detached dispatch)

> 评审对象:`prd.md` / `design.md` / `implement.md`(2026-08-29 规划态,代码零改动)。
> 评审方式:PRD「代码取证结论」逐条对源码核实 + 设计定案对既有契约(spec / 过滤链 /
> 权限层)交叉验证。评审日期:2026-08-29。

## 总评

**规划质量高,方向正确,但有 1 个 P1 设计错误必须先修再实施。**

PRD 的代码取证几乎全部属实(行号精确):`_inner` 校验矩阵、`created_by` 硬编码与
db 注释预告、kill switch fail-open、Tier 5 fallthrough、prefix cache 尾部追加惯例、
`ToolContext` 自带 `db` + `project_id`、`ScheduleSpec` 6 档、F2b `ends_at` 当日仍触发
语义——全部核实为真。复用边界(「fire 链零改动」)划得干净,AC 可测,回滚分析成立。

主要问题:**design D5 群聊隔离一条把过滤方向写反了**——照做会把三具从普通 chat
剥除、群聊反而保留,恰好毁掉整个特性(详见 P1)。另有一个可预见的测试红
(stub 静态预算)未列入实施步骤(P2-1)。

## P1 — D5 群聊隔离机制方向写反(必须修)

**design D5 第二条**:「`filter_tools_for_session_type`(tools/mod.rs:311)的**非群聊
剥除名单**追加三名(镜像 `nominate_speaker` 处理)」;PRD R3 同款表述。

**实际实现**(`tools/mod.rs:311-320`):

```rust
if is_group_chat {
    return tools;                    // 群聊:原样全量返回
}
tools.into_iter().filter(|t| !matches!(t.name.as_str(),
    "nominate_speaker" | "end_discussion")).collect()   // 非群聊:剥除名单
```

该名单是「从**非群聊**剥除」的名单(nominate_speaker / end_discussion 是群聊专属
仲裁工具,从普通 chat 剥掉)。把三具加进去 = 三具从普通 chat 消失、群聊反而可见
(群聊分支不过滤)——与 AC4(「群聊 toolset 均无三工具;非 worker 普通 chat 可见」)
完全相反。

**且群聊侧本就零改动即满足**:群聊工具集由 `group_chat_tool_defs`
(`agent/group_chat_prompts.rs:216`)的**穷举白名单**构建
(`GROUP_CHAT_RESEARCH_TOOLS` + moderator 两件),注释明示「白名单是穷举的:新注册
`builtin_tools` 条目**不会**自动进入群聊」——这正是 08-07 从黑名单改白名单的教训
(DB session `8be4687f` 弱模型滥用泄漏工具)。三具新注册 → 群聊天然不可见。

**修正建议**:

- D5 删掉该条,改为:「群聊隔离 = `group_chat_tool_defs` 白名单天然排除(**零改动**)
  + 补 AC4 断言测试(moderator / participant 两态 toolset 均无三具)」。
- 执行侧防御不变:`in_current_session=true` 在群聊本不可达(工具不可见);
  `_inner` 的 `validate_target_session` 拒群聊作为纵深防御照旧成立。
- AC4 / implement.md 步骤 4 的「`filter_tools_for_session_type` 名单追加」同步删除,
  测试落点改为 `group_chat_tool_defs`。
- 不必反向去改 `filter_tools_for_session_type` 的群聊分支(early-return)——其模块
  注释明确「不得越俎代庖 second-guess 群聊白名单」。

## P2 — 应补事项

### P2-1 stub 静态预算测试必红,implement.md 无对应步骤

`static_token_budget_classic_chat_first_turn`(`tools/stub.rs:330`)锁 classic-chat
首轮 stubified `tools[]` ≤ **3960** tok,当前实测 ~3903(校准注 4)。注册三具 +
STUB_CANDIDATES 扩员后,按校准史(10 个 stub 含 JSON 包装 ≈ 330 tok,~33/具)预计
+~100 tok → ~4000,**超线**。

校准注 3 的既定政策正是「若未来再注册新工具逼近线,**优先扩 STUB_CANDIDATES** 而
非继续平移」——本任务就是该情形,按政策线随 stub 增量平移(约 3960 → ~4070)+
补校准注 5。**implement.md 步骤 4 应补此步**,否则步骤 4 提交时测试红,实施者会
误判为回归。机械连带:`STUB_CANDIDATES: [&str; 11]` 与
`STUB_DESCRIPTIONS: [(&str, &str); 11]` 是定长字面量,长度同步 11 → 14。

### P2-2 R4 上限的 TOCTOU 窗口(备注即可,不必修)

cap 是 COUNT → INSERT 两步,同 project 两个并发 agent turn(不同 session 并行)可
双双通过 gate,瞬时 21+。单用户桌面 + 20 本身是反滥用软上限,危害可忽略;但建议
implement 步骤 3 的 cap 测试注释写明「窗口竞态有意接受,上限为软语义」,防止未来
被当 bug 误修(或加无谓的原子化 SQL)。

## P3 — 文档小修

1. **路径/行号漂移**:`subagent/tools_filter.rs` 实为 `agent/subagent/tools_filter.rs`
   (PRD Background 7 / design D5);`STRUCTURALLY_DISABLED` 在 :24(design 写 :30)。
   `filter_tools_for_subagent` :80 属实。
2. **implement.md 验证命令 shell 语法错误**(照抄必踩坑):

   ```bash
   cargo test -p everlasting --lib \
     PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
   ```

   续行后 `PKG_CONFIG_PATH=...` 成了 cargo 的**位置参数**(被当过滤串),env 不会
   生效,WSL 上必然复现 gdk-pixbuf 找不到。应为
   `PKG_CONFIG_PATH="..." cargo test -p everlasting --lib`(下方 clippy 行写对了,
   上面这行抄时反了)。
3. **AC2「群聊 target → 结构化错误」触发路径不存在于 agent 面**:R7 已定案不暴露
   `target_session_id`,工具面只有 `in_current_session`;群聊会话又看不见三具。该
   错误路径实际只能经用户 IPC(`create_scheduled_task` 指定群聊 session)触发——
   那是 F2 既有覆盖,不是本任务 AC。建议 AC2 此项改为「经 `_inner` 直测(既有
   路径回归)」或删去,免得实施者找不到 agent 侧触发方式。
4. **R7 理由略过强**:「LLM 无从得知其他 session 的 id」不成立——`search_history`
   结果原样展示非当前 session 的 id(`tools/search_history.rs:189` 只给当前
   session 打 `(this session)` 标)。结论(不暴露裸 `target_session_id`)仍对:
   主场景 `in_current_session` 已覆盖 + 简化 schema 防误用;建议理由改述,避免
   误导后续人以为 id 是保密的。

## 已核实为真的关键前提(给实施者的信心清单)

| PRD/设计声称 | 核实结果 |
|---|---|
| `create_scheduled_task_inner` @ `commands/scheduled_tasks.rs:122`,校验矩阵全量 | ✅ 逐条相符(name/prompt/结束条件/parse_schedule/project/target session 存在+chat+归属) |
| db:108 注释预告 `created_by` 参数化,SQL 硬编码 `'user'`(:120) | ✅ 原文在 |
| kill switch fail-open,仅字面 `"false"` 关(`scheduler/mod.rs:229-238`) | ✅;创建侧确无检查 |
| `classify_tool` 不加 arm → `ToolKind::Other` = Tier 5 Allow + 自动 `ToolAllowed` 审计 | ✅ `permission.rs:560-567, 594-614` |
| `builtin_tools()` 尾部追加 = prefix cache 契约 | ✅ web_search 条目注释明示「Appended LAST (prefix cache)」 |
| worker 剥除走 `STRUCTURALLY_DISABLED`(`create_task` 同款「编排面归 parent」理由) | ✅ `agent/subagent/tools_filter.rs:24-70` |
| Plan mode 不剥三具 | ✅ `permissions/mode.rs:52-73` 是黑名单制,新工具默认保留(同 `remember`) |
| `READONLY_TOOL_ALLOWLIST` 不加 → 并发只读 worker 不可见 | ✅ 白名单制,天然排除 |
| `ToolContext` 自带 `db` + `project_id` | ✅ `tools/mod.rs:369-383`(cap COUNT / 项目过滤 / kill switch 读取全可得) |
| execute 签名可拿 `session_id`(`Option<&str>`) | ✅ `search_history::execute` 先例 |
| `ScheduleSpec` 恰 6 档 | ✅ `scheduler/compute.rs:44-60` |
| `ends_at` 转当日 23:59:59.999 与 F2b「结束日当天仍触发」一致 | ✅ gate 是 `due > ends_at` 才跳过(`scheduler/mod.rs:286`) |
| fire 链与 `created_by` 无关 → AC1「fire 不可区分」 | ✅ `TaskOrigin::Scheduled` 仅 task_id/name/fired_at(`mod.rs:200-205`) |
| daemon route 包装存在(改签名波及 2 处生产调用点) | ✅ `daemon/routes/scheduled_tasks.rs:50` + Tauri command;route 文件内自带测试 |
| db 单测 `created_by=='user'` 断言(:423)改签名后仍成立 | ✅ 调用点显式传 `"user"` 即可 |
| 前端落点真实 | ✅ `ScheduledTasksTab.vue` + `stores/scheduledTasks.ts` 存在 |
| spec 三份引用 | ✅ `scheduled-tasks.md`(schema 含 created_by DEFAULT 'user')/ `tool-contract.md`(编号条目制)/ `permission-layer.md`(Tier 5 默认 allow-all)均在且相符 |

## 产品/安全面记录(不复议用户定案)

Q2 silent Allow + Q3 上限 20 的补偿控制链完整:创建零立即副作用、fire 走完整
mode/permission 链、Settings 可见可删可禁、kill switch 急停语义闭环(R5 错误信息
指引关 `scheduled_tasks_enabled`,关掉后存量 fire 同停——scheduler_tick 同键
fail-closed)。一个值得意识到的残余:agent 建的 interval 任务每次 fire 是一轮真实
LLM 调用(花钱),20 上限 × 任意频率 = 潜在成本面——「不限制调度频率」已明确
留给 F3 资源治理,此处仅记录,不要求本任务处理。

## 结论

**有条件放行**:先修 D5(P1,群聊隔离改为「白名单天然排除 + 测试」,删
`filter_tools_for_session_type` 名单追加),同步修 AC2/AC4 措辞与 implement.md
步骤 4(补 stub 预算线平移);P3 文档项顺手修。P1 修复是纯文档改动,不动任何
代码边界,修完即可按 implement.md 步骤 1 开工。
