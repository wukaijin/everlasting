# Design — A2+ P3c 沙盒 UX 增强档

> 依据:prd.md(D1–D4 已裁定)+ P3b spec `sandbox-executor.md`(规则集契约不变,
> 本任务只动触发/面/审批语义)。源方案 §4。

## 1. 总体:决策链重构

P3b 的 `gate`(四项与:capability / ReadOnly 档 / mode≠Yolo / enabled)升级为
**单一决策函数** `resolve_policy`,权限层与执行层共用同一真源:

```
resolve_policy(mode, project_policy, kill_switch, capability) -> Policy
  Policy ::= Off                    // 不沙盒,经典 Tier 4 路径
           | Face(ReadWrite)        // 全命令沙盒,worktree 可写
           | Face(ReadOnly)         // 全命令沙盒,worktree 只读

求值顺序(短路,配置读惰性——SBX-004 精神):
  1. capability 探测失败        -> Off            // fail-open(不变)
  2. mode == Yolo               -> Off            // 恒不沙盒(不变)
  3. project_policy == Off      -> Off            // 零额外读
  4. kill_switch == false       -> Off            // master 开关
  5. mode == Plan               -> Face(ReadOnly) // session 级只读面
  6. project_policy             -> Face(其档位)
```

关键语义(PRD D2/D3):

- **Plan 恒只读面**:项目档位在 Plan 下被 session 覆盖(5 优先于 6);但项目
  `off` 或链路不可用时 Plan 整体回退工具过滤(§4),不落到「Plan + 弹窗放行写」。
- **优先级链**:capability → Yolo → 项目档 → kill-switch → mode。kill-switch
  是 master(关 = 全局 `Off`,含只读档项目)。
- `classify_prefix` 不再参与沙盒触发(全命令触发),判定层语义零改动(PRD C2
  延续);`off` 档项目里它照旧服务 Tier 4。

### 1.1 决策点分布(两处消费,一处真源)

| 消费点 | 位置 | 消费方式 |
|---|---|---|
| Tier 4 shell 分支短路 | `permissions/check/permission.rs` shell 分支头 | `Policy != Off` → 跳过 prefix-grant / 三档分类 / ask,直接 `Allow`(记 `ToolAllowed` 审计);`Off` → 原路径逐字节不变 |
| spawn 侧沙盒 | `sandbox::decide`(mod.rs:222) | `Policy != Off` → `prepare(spec(face))` + `apply`;`Off` → 不设 pre_exec |

- 短路点在 **Tier 1–3 之后**(硬拒/敏感路径层不被取代,PRD D2 不变量);
  实施时核对 check() 内危险/敏感检查确实先于 Tier 4 shell 分支。
- 项目档读取:`read_project_sandbox_policy(db, session_id)` 经
  `sessions.project_id` join `projects`(索引点查;避免给 `PermissionContext`
  加字段)。两个消费点各自调,点查开销可忽略;不跨层传递 Decision。

## 2. 数据与配置(D5)

**载体:projects 新列**(弃 metadata JSON——类型化 CHECK 不变式优于自由 JSON):

```sql
ALTER TABLE projects ADD COLUMN sandbox_policy TEXT NOT NULL
  DEFAULT 'readwrite'
  CHECK (sandbox_policy IN ('off', 'readwrite', 'readonly'));
```

- 迁移走既有 migration 通道(ADD COLUMN + 常量 DEFAULT 合法);若迁移基建仅
  支持表重建,按 scheduled-tasks 先例改重建,语义不变。
- **默认 `readwrite` 即行为变更**(PRD D2 明示):存量项目升级后全命令进沙盒。
  回滚 = kill-switch 关,或单项目切 `off`。
- 读写命令:`update_project_sandbox_policy(project_id, policy)`(daemon route +
  Tauri command 双端 additive);读侧 `get_projects` / session 装载链带出字段。
- 前端:项目编辑面加三态选择(放行/读写/只读 + 一句话说明);GeneralTab 全局
  kill-switch 面板保持现状。

## 3. 只读面 spec 变体

`policy::build_spec(ctx, session_id, extra, face)` 增面参数:

- **ReadWrite 面** = 现状:writable = worktree + /tmp + spill + extras。
- **ReadOnly 面**:writable = /tmp + spill + extras(**worktree 移出**);
  exec 面显式补回 worktree(项目脚本仍可运行——policy.rs:163 现在靠
  `extend(writable_roots)` 间接获得,拆开后需显式追加)。
- extras(`~/.cargo` 等)两面均保留可写:用户显式授予的全局目录,与项目面
  正交;`/tmp` 逃生口两面保留(`CARGO_TARGET_DIR=/tmp/...` 调查型构建,D3)。
- `SandboxSpec::summary()` 增 `face=ro|rw` 段(审计 payload,两路同形)。

## 4. Plan 模式:tool list 与回退

`filter_tools_for_mode`(mode.rs:52)签名扩为
`(tools, mode, plan_shell_available: bool)`:

- `Plan && !plan_shell_available` → 照今天滤掉 shell 族(回退路径)。
- `Plan && plan_shell_available` → 保留 shell 族(其余 write 工具照滤)。
- Edit/Yolo:不受此参数影响。

`plan_shell_available` 在 drive.rs:318 调用点按轮计算:
`resolve_policy(Plan, project_policy, kill_switch, capability) != Off`
——一次点查(该轮 Tier 4 短路与 spawn 侧会各自再算,接受;轮顶这次只为
tool list)。**非 Linux 恒 false**(探测恒败)→ 行为 = 现状,C4 自动保持。

Tier 4 休眠 Plan 分支(permission.rs:423 SideEffect→Ask)保持休眠:Plan 下
`Policy != Off` 时被 §1.1 短路,`Policy == Off` 时工具已被滤掉,两路都到不了。

## 5. 升级闭环(前台 shell,PRD D4)

### 5.1 触发与防抖

```
sandbox_applied && exit≠0 && mode != Plan && stderr 命中保守特征:
  写拦截: "Permission denied" | "Read-only file system"
  断网:   "Operation not permitted"
```

- 保守匹配(P3b 宁缺勿滥原则);误报代价 = 一张可拒绝的卡(卡面带 stderr
  原文行,用户可判断);漏报退化为指引文案,不放大。
- **Plan 不升级**(D3):命特征只追加模式感知指引。
- **每 tool call 至多一次**:批准重跑(不沙盒)若再失败,按普通失败返回,
  不再升级(防循环)。

### 5.2 流程

```
命中特征
  ├─ prefix-grant 命中(同 Tier 4 闸:has_structural_metachar 复合命令不享受,
  │   first_token_for_allow_always 精确匹配)→ 直接不沙盒重跑 + 审计
  └─ 未命中 → 复用 ask_path(ask.rs:136)弹卡:
       payload = 原命令文本 + 拦截原因(面外写/断网)+ stderr 原文行
       ├─ AllowOnce  → 不沙盒重跑原命令(逐字节同 command/env/cwd)
       ├─ AllowAlways→ 写 prefix-grant(既有通道,kind↔类别矩阵校验)+
       │               不沙盒重跑;后续同前缀命中 §5.2 顶部分支(AC6)
       └─ Deny       → 失败结果 + 指引回模型
```

- **审批绑定确切命令文本**:重跑的是用户所见命令,不经模型转述(审计干净)。
- **双重执行边界**(D4 已接受):升级仅在面外写/断网被拒时触发——第一遍的
  危险部分未发生;面内写重跑与今天批准执行同类。
- `ask_path` 依赖 sink/db/store/PermissionContext/tool_use_id/token:shell 工具
  现有 `ToolContext` 不含 sink 与 store——经 **`ToolContext` 扩展一个
  `escalation: EscalationHandle`(Option,sink+store+ctx 子集)**,由 dispatch
  构造灌入(对齐 P3b D5 的 mode 灌入先例);测试路径 None = 升级退化为指引。
- worker 路径:ask 机制本就支持 worker ask(ask.rs 注释三条件),升级卡在
  worker 内同样成立,无需特判;run_grants 命中路径与 Tier 4 现状一致。

### 5.3 指引文案参数化

`write_block_guidance`(mod.rs:423)拆两变体:

- Edit 档:「写入被沙盒面拦截……可批准升级(若收到升级卡)或让用户调整
  `sandbox_extra_writable` / 项目档位」。
- Plan 档(D3):「你在 Plan 模式,写入被拦截是设计使然;请给 diff 提案并请
  用户切 Edit,或把中间产物写到 /tmp」。
- 断网特征单独文案(「沙盒断网……如需联网操作请触发升级审批/切换档位」),
  不再混入写指引。

## 6. 三条 P3 债的落点

- **RULE-SBX-002**:config store 分离 `sandboxExtraWritable`(raw,可编辑)与
  展示层 effective(raw + 后端并入的 `~/.cargo`);GeneralTab 默认项渲染为
  固定 chip(无移除按钮)+ raw 列表可增删;修正失真注释。
- **RULE-SBX-003**:`run_background_shell.rs` 的 `record_sandboxed_shell_audit`
  块挪到 `registry.start` Ok 分支之后(与前台路径口径一致)。
- **RULE-SBX-004**:由 §1 决策链的惰性求值顺序结构性解决(config 读在
  capability/Yolo/off 判定之后);DEBT 销账时引用本节。

## 7. 兼容与回滚

- wire 全部 additive:projects 字段、审计 payload 新段、`plan_shell_available`
  为内部签名不改 wire。旧客户端读新 DB:未知列忽略,行为 = 旧(仅
  `get_projects` 多一字段)。
- **回滚面**:全局 kill-switch 关 → 全链路 `Off` = P3b 前行为(Edit 经典
  Tier 4、Plan 工具过滤);单项目回滚 = 切 `off`。升级闭环不依赖任何新表。
- 非 Linux:`resolve_policy` 恒 `Off`(探测败)→ 全部现状行为,C4 保持;
  RULE-SBX-001 不在本期(触发条件未到)。
- 判定层(`shell_trust.rs` / Tier 1–3)零改动——`off` 档与全部回退路径的
  行为锚点。

## 8. 已识别风险

| 风险 | 缓解 |
|---|---|
| 读写档默认变更伤及存量体验(build 断网依赖变多一跳) | 升级卡 AllowAlways + prefix-grant 记住;turn-smoke live 验证无误杀 |
| 升级特征误报(chmod 000 读失败弹卡) | 保守串匹配 + 卡面带 stderr 原文;漏报仅退化为指引 |
| Tier 4 短路点与 Tier 1–3 顺序 | 实施首步核对 check() 内序;集成测试钉死(fork bomb 在 readwrite 档仍拒) |
| `ask_path` 复用的参数面宽(sink/store/token) | EscalationHandle 打包注入,测试路径 None 降级 |
| 双决策点漂移(Tier 4 与 spawn 各算一次) | 单一 `resolve_policy` 真源 + 矩阵单测锁两消费点一致性 |
