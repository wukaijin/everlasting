# Implement — LLM schedule_task tool(detached dispatch)

> 依赖:`design.md` 定案 D1–D9。按步提交,每步可独立编译 + 该步测试绿。

## 步骤(实施序)

1. **db 层参数化**(D2)
   - `db/scheduled_tasks.rs`:`NewScheduledTask` 增 `created_by: String`;INSERT 改绑参;
     `list_scheduled_tasks` 增 `created_by: Option<&str>`。
   - 既有调用点显式传 `"user"` / `None`(`create_scheduled_task_inner` + db 单测)。
   - 测:created_by 落库断言(agent 值可写入)、list 过滤正负向;既有
     `created_by=='user'` 断言不动。
2. **`create_scheduled_task_inner` 增 created_by 参数**(D2/D3)
   - 签名加 `created_by: String` 透传 `NewScheduledTask`;Tauri command 与 daemon route
     两个包装传 `"user"`(parity 测试同步)。
   - 测:既有 create 校验矩阵全绿(行为零变化回归)。
3. **新模块 `tools/scheduled_task_family.rs`**(D1/D3/D6/D7)
   - 三 `definition()` + 三 `execute`;常量 `MAX_ACTIVE_AGENT_TASKS=20`、kill switch
     键共享常量(抽到 `scheduler/mod.rs` 或 db config 侧,`scheduler_tick` 与 tool 同源)。
   - kill switch → 上限 COUNT → `_inner` 调用顺序;`schedule_cancel` 先查
     created_by 再删;`ends_at` ISO→epoch ms 转换 + 错误矩阵。
   - 测:validation 短路矩阵(AC2)、in_current_session(AC3,mock pool)、
     kill switch(AC5)、上限触顶/自愈(AC6;注释写明 TOCTOU 竞态有意接受,
     上限为软语义 —— 评审 P2-2)、cancel 越权/幂等(AC7)。
4. **注册 + 隔离 + stub**(D1/D5/D8)
   - `tools/mod.rs::builtin_tools()` 尾部追加三 definition;`STRUCTURALLY_DISABLED`
     (`agent/subagent/tools_filter.rs:24`)追加三名。**群聊侧零改动**
     (`group_chat_tool_defs` 白名单天然排除;勿动 `filter_tools_for_session_type`
     —— 方向相反,评审 P1)。
   - `STUB_CANDIDATES` 与 `STUB_DESCRIPTIONS` 同步 11 → 14;实测
     `static_token_budget_classic_chat_first_turn` 新值后平移预算线(3960 → 实测值
     +余量)并补校准注 5(D8)。
   - 测:既有不变量单测(stub∩并行白名单)自动覆盖;补 filter 断言(worker 无三具)
     + 群聊两态断言(`group_chat_tool_defs` moderator/participant 均无三具)(AC4)。
5. **前端徽标**(D9)
   - `ScheduledTaskPayload.created_by` + Settings 定时任务 tab agent chip;
     `pnpm test` 补 payload 映射/徽标渲染用例。
6. **集成 + 回归**
   - 集成:mock LLM 循环调 `schedule_task` → 落库 `created_by='agent'`(镜像
     `tests_request_mode_change.rs` 组织);fire 链回归由既有
     `scheduler/tests_tick.rs` / `tests_message_queue.rs` 零改动通过背书(AC1 后半)。
   - 全量:`cargo test -p everlasting --lib` + `pnpm test` + lib clippy + fmt。
   - spec 收口:`scheduled-tasks.md`(created_by=agent 创建路径 + 上限/kill switch
     创建侧契约)、`tool-contract.md`(三具条目)、`permission-layer.md`(Tier 5 归位
     一句话);DEBT 无涉。
7. **live 冒烟(可选但推荐)**
   - daemon 起后经 turn-smoke 或真实对话让 agent 建一个 1 分钟 interval 任务 →
     Settings 可见 → 到期 fire → `schedule_cancel` 删。覆盖 AC1 端到端。

## 验证命令

```bash
# 后端(WSL 需 PKG_CONFIG_PATH;env 赋值必须在命令**前**,续行放后面会变成 cargo 位置参数)
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib
# 前端
cd app && pnpm test
# lint
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  sh -c 'cd app/src-tauri && cargo clippy --lib && cargo fmt --check'
```

## 风险点 / 回滚

- `builtin_tools()` 追加顺序破坏 prefix cache → 只准尾部 append(code review 自查项)。
- `insert_scheduled_task` 改签名波及面:全库 grep 调用点(预期 2 处生产 + db 单测)。
- filter 名单漏一个名字 → AC4 单测抓住;STRUCTURALLY_DISABLED 有既有不变量测试模式
  可镜像。
- 回滚:每步独立 commit,revert 对应 commit 即可;`created_by='agent'` 存量行 revert
  后无害(design §5)。
