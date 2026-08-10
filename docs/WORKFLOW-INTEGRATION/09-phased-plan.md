## 9. 分阶段实施计划

> 排期归 [ROADMAP.md](../ROADMAP.md)。本节是**功能落地依赖拓扑**,每阶段独立可验证。

### Phase 0:engine 骨架(state 注入 + workflow 开关)

**目标**:workflow 开关 + state machine + breadcrumb 注入跑通,验证"agent 读 breadcrumb 改变行为"。

- `sessions` 表加 `workflow_enabled` 列 + migration
- 顶栏 workflow toggle(B7 mode 切换同级)
- engine 读"硬编码默认 plugin"(dev 四态,架构留位待 Phase 2 外置)
- task.json 读写 + `.everlasting/tasks/<slug>/` 目录约定
- state breadcrumb 注入(复用 `inject_recall_into_turn` append seam)
- agent 在 workflow session 自动起/续 task
- state 转移用户确认门(agent ask_user_question 带 purpose + 专用 IPC `resolve_task_state_transition`,M-A)

**完成标准**:开 workflow → agent 自动起 task 写 task.json → 推进 state(用户确认)→ breadcrumb 随 state 变化可见。

### Phase 1:skill 规范包 + plugin skill loader

**目标**:"规范"落地,agent 知道"标准做法";plugin skill 加载路径就位(避免 Phase 2 迁移债,M4 决定4选 b)。

- **plugin skill loader**:`skill/loader.rs` 加 workflow plugin skills 解析层(`.everlasting/workflow/<plugin>/skills/*/SKILL.md`),解析顺序 plugin skills → project 全局 → user 全局 → builtin(见 §6.3)。即使 Phase 1 时 workflow.json 还是硬编码,skill 走 plugin 目录
- 5 个 wf-* skill(wf-overview / wf-brainstorm / wf-before-dev / wf-check / wf-update-spec,借鉴 Trellis 同名,**去 Trellis Python hook 依赖换成 Everlasting Rust hook 触发**,纯描述性);放 plugin 目录 `.everlasting/workflow/dev/skills/`
- artifact 查阅机制:task.json 元数据+summary 进 messages[0];prd/design/progress 给 path

**完成标准**:进 workflow 时 agent use_skill wf-overview 建立全局意识;planning state agent use_skill wf-brainstorm;各 skill body 被读到。

### Phase 2:plugin 外置 + sub-agent 角色 + 上下文注入

**目标**:engine/content 真分离;调研/实施/验收真分工;worker 看得到 artifact。

- 硬编码默认外置成 `.everlasting/workflow/dev/workflow.json`;engine 改读配置驱动(`default_workflow()` → `load_workflow()`,engine 主体零改动,见 §5.4)
- plugin agents/ 落地(Q5:plugin 自带 researcher/implementer/checker 三角色;builtin general-purpose 不动)
- state 门控 dispatch(统一协商档,见 §5.2.3 + §6.6.2;**执行点下沉 `run_subagent` 内部**,S-A)
- worker 上下文注入(Q6:delegation 模板,engine 填 task meta 占位符 + 主 LLM 填委托细节)
- checklist 同步(S2 选 c):`update_checklist` 在 workflow session 内改写 task.json.items(非 loop-local Vec);B12 coerce 逻辑保留;非 workflow session 行为不变
- UI:plugin 切换(此时只有一个 plugin,但切换机制就位)

**完成标准**:改 workflow.json 能改流程;planning 只派 researcher;worker 收到的 delegation 带 task meta + 角色模板框架;checklist items 跨 session 持久(task.json)。

### Phase 3:hook + 沉淀闭环

**目标**:task done 自动沉淀 spec;长跑积累项目规范。

- state 转移 Rust 固定 hook(Q9 选 b:done 时触发沉淀)
- `.everlasting/spec/` 目录 + wf-update-spec 落地
- progress.md 交接叙述
- task 归档(done → `.everlasting/tasks/archive/`)

**完成标准**:task done → spec 写进 .everlasting/spec/;下次 implement 读得到;跨 session 续 task 接上。

### Phase 4+:第二个 plugin `review`(评审流)+ B8 衔接(愿景/远期)

- review 作为第二个 workflow.json(Q8:回合制 A 可直接做;实时群聊 B 需新通讯原语,独立立项)
- B8 DAG(归 ROADMAP 第四档)

> Phase 4 不在 MVP 范围;本文档只记愿景,设计届时展开。

### 9.5 分步实施指导(每个 Phase 拆成可独立验证的小步)

> 上面的 Phase 0-3 是粗粒度阶段划分。实际开发时每个 Phase 体量不小(尤其 Phase 1/2),拆成可独立验证、可独立提交的小步,降低风险 + 加快反馈。每步标注**验证手段**(怎么确认这步做对了)和**依赖**(必须先完成哪步)。
>
> 每个小步 = 一个 git commit + 一个可复现的验证(测试 / 手动操作 + 预期)。**不累积多步一起提交**——出问题好回滚。

#### Phase 0 拆步(5 步,每步独立提交)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **0.1** | `sessions` 表加 `workflow_enabled` 列 + migration;Pinia store + Tauri command 读写 | cargo test migration 通过;前端读得到字段 | — |
| **0.2** | 顶栏 workflow toggle UI(B7 mode 切换同级组件)+ 绑 store | 手动点 toggle,DB 列翻转;关 session 重开状态保持 | 0.1 |
| **0.3** | `WorkflowDef` struct + 4 访问函数 + `default_workflow()` 硬编码常量(dev 四态)——**纯 Rust,无 UI** | cargo test:`breadcrumb_for(planning)` 返回期望文本;`can_transition(planning, implement)=true` | — |
| **0.4** | task.json 读写 + `.everlasting/tasks/<slug>/` 目录约定 + `create_task` command(§6.8) | 手动调 create_task → 目录 + 文件生成,JSON 合法;读回来字段对 | 0.3 |
| **0.5** | state breadcrumb 注入(复用 `inject_recall_into_turn` append seam)+ agent 在 workflow session 自动起/续 task(§6.2 触发点) | 集成测试:workflow session 发开发意图消息 → task.json 生成 → breadcrumb 随 state 变化出现在 messages[0] | 0.2, 0.4 |

**Phase 0 完成标志**:0.1-0.5 全过 → 开 workflow → agent 起 task → breadcrumb 可见。

#### Phase 1 拆步(4 步,体量大但每步聚焦)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **1.1** | plugin skill loader(`skill/loader.rs` 加 workflow plugin skills 解析层,M4 选 b) | cargo test:plugin skills 优先于全局解析;无 plugin 时 fallback 全局 | Phase 0 |
| **1.2** | wf-overview skill body 完整(§A.4)——这一个先做,是 agent 进 workflow 的入口 | 手动:workflow session agent use_skill wf-overview → body 返回 | 1.1 |
| **1.3** | wf-brainstorm / wf-before-dev / wf-check / wf-update-spec 4 个 skill body(§A.5 outline 填肉) | 各 skill 被对应 state 的 agent 加载 | 1.2 |
| **1.4** | artifact 查阅机制:task.json 元数据+summary append messages[0];prd/design/progress 给 path | 集成测试:messages[0] 含 task meta;agent read_file prd 成功 | 1.2 |

**Phase 1 完成标志**:进 workflow agent 读 wf-overview;planning 用 wf-brainstorm;各 skill body 可读;task meta 注入可见。

#### Phase 2 拆步(6 步,最重,分两批)

**批 A:plugin 外置 + 接口换源(2 步)**

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **2.1** | workflow.json 外置:`default_workflow()` → `load_workflow()` + validate + fallback(M6) | cargo test:改 workflow.json states → breadcrumb 跟着变;malformed JSON → fallback default + warn | Phase 1 |
| **2.2** | UI plugin 切换(顶栏 plugin 名点击 → 切换;此时只有 dev 但机制就位) | 手动切 plugin → breadcrumb 重新注入 | 2.1 |

**批 B:sub-agent 角色 + 门控 + 注入(4 步)**

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **2.3** | plugin agents/ 落地(researcher/implementer/checker frontmatter + body,§A.3);loader 加 plugin agents 解析层(Q5) | cargo test:workflow session 解析 implementer 用 plugin 的;非 workflow 用全局 | 2.1 |
| **2.4** | 门控执行点(§6.6.2):**下沉 `run_subagent` 内部**加 role×state 校验 + 协商档(S-A:三处调用点都过);签名追加 `current_state` 参数 | 集成测试:planning 派 implementer → 弹协商;并发 dispatch path 也拦截;用户允许 → 放行;拒绝 → breadcrumb | 2.3 |
| **2.5** | delegation 模板注入(§6.6.1):engine 填 task meta 占位符 → append messages[0] | 集成测试:dispatch 时 messages[0] 含填好的 delegation 模板;worker 读到角色规范 | 2.3 |
| **2.6** | checklist 同步(S2):`update_checklist` workflow session 内改写 task.json.items;B12 coerce 保留 | 集成测试:跨 session 续 task → items 进度持久;worker 读 task.json 拿进度 | 2.3 |

**Phase 2 完成标志**:改 workflow.json 改流程;planning 只派 researcher;delegation 模板注入;checklist 跨 session 持久。

#### Phase 3 拆步(3 步)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **3.1** | state 转移 Rust 固定 hook(§8):`set_task_state` 加 match 分支;user 确认门 resolve handler 自动调(M2) | 集成测试:Check→Done 触发 hook;planning→implement 触发 preflight | Phase 2 |
| **3.2** | `.everlasting/spec/` 目录 + wf-update-spec 落地(沉淀闭环) | 手动:task done → spec 文件生成;下次 implement 读得到 | 3.1 |
| **3.3** | progress.md 交接叙述 + `archive_task` command(§6.8) | 手动:done → task 移到 archive/YYYY-MM/;新 session 读 progress 续上 | 3.2 |

**Phase 3 完成标志**:done 自动沉淀 spec + 归档;跨 session 续 task 完整闭环。

#### 总步数 + 依赖图

```
Phase 0 (5 步):  0.1 → 0.2 ─┐
                  0.3 → 0.4 ─┤→ 0.5
                             ↓
Phase 1 (4 步):  1.1 → 1.2 → 1.3
                         ↘ 1.4
                             ↓
Phase 2 (6 步):  2.1 → 2.2          (批 A)
                  2.3 → 2.4 / 2.5 / 2.6  (批 B,2.4/2.5/2.6 并行)
                             ↓
Phase 3 (3 步):  3.1 → 3.2 → 3.3
```

**关键并行点**:Phase 2 批 B 的 2.4(门控)/ 2.5(delegation 注入)/ 2.6(checklist)三步都只依赖 2.3,可并行做。

**风险最高的步**:2.4(门控执行点,**下沉 `run_subagent` 内部**,改签名 + 三处调用点,S-A)、2.6(checklist 同步,改 B12 写路径)——这两步碰现有稳定代码,做完立即跑全量 cargo test + vitest。

---
