# Implement — A2+ P3c 沙盒 UX 增强档

> 四 PR 串行,每 PR 独立可合(提交即绿)。inline 工作流,Phase 2 经
> trellis-before-dev 装载上下文;implement.jsonl/check.jsonl 不作门禁。

## PR1 — 决策链重构 + 三态(纯后端)

- [ ] 1.1 核对 `permissions::check` 内 Tier 1–3(危险/敏感)先于 Tier 4 shell
      分支(design §1.1 前置核对;不符则短路点相应上移并记 design 修订)。
- [ ] 1.2 `projects` 迁移:`sandbox_policy` 列(design §2 SQL)+ 读侧带出
      (`get_projects` / session 装载链)。
- [ ] 1.3 `sandbox::resolve_policy` 真源(惰性求值顺序 = design §1)+
      `read_project_sandbox_policy(db, session_id)` join 读取。
- [ ] 1.4 `gate`/`decide` 重接 `resolve_policy`(删 ReadOnly 档触发;Yolo/
      capability 语义不变);SBX-004 随此结构性解决。
- [ ] 1.5 `build_spec` 面参数(readonly 面:worktree 出 writable、显式进 exec);
      `summary()` 增 `face=` 段。
- [ ] 1.6 Tier 4 shell 分支短路(`Policy != Off` → Allow + 审计;`Off` 原路径)。
- [ ] 1.7 单测:resolve_policy 全矩阵(policy × mode × kill-switch × capability)、
      面构造、Tier 4 短路/不短路、真内核集成扩展(readonly 面 worktree 写拒 /
      exec 项目脚本过 / 读写面 worktree 写过;fork bomb 在 readwrite 档仍静默拒)。
- [ ] 1.8 审计:payload face 段 roundtrip。

验证:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 照旧)+ clippy + fmt。

## PR2 — Plan 模式沙盒化

- [ ] 2.1 `filter_tools_for_mode` 增 `plan_shell_available` 参数 + drive.rs:318
      调用点按轮计算(一次 `resolve_policy` 点查)。
- [ ] 2.2 回退路径测试:kill-switch 关 / 项目 off / 非 Linux(探测桩)→ Plan
      滤 shell 族(现状);链路可用 → 保留。
- [ ] 2.3 指引文案参数化(design §5.3:Edit/Plan/断网三变体)+ 单测钉文案要点。
- [ ] 2.4 SBX-003:后台审计块挪 `registry.start` Ok 后。
- [ ] 2.5 集成:Plan 全命令只读面 + 无升级(命特征仅指引)+ Yolo 不沙盒。

验证:同 PR1 + `tests_mode` 家族全绿。

## PR3 — 升级闭环(前台)

- [ ] 3.1 `EscalationHandle`(sink+store+ctx 子集)进 `ToolContext`,dispatch
      灌入(先例:P3b D5 mode 灌入);测试路径 None。
- [ ] 3.2 触发检测(§5.1 保守特征 + 每 call 一次闸 + Plan 排除)。
- [ ] 3.3 prefix-grant 先查(复合命令闸同 Tier 4)→ 命中直接不沙盒重跑。
- [ ] 3.4 `ask_path` 复用弹卡(命令 + 拦截原因 + stderr 行);AllowOnce/
      AllowAlways(写 grant,kind↔类别矩阵)/ Deny 三分支 + 重跑不沙盒。
- [ ] 3.5 审计落位(ask 侧既有 kinds + 重跑行);幂等边界注释(双执行范围,
      design §5.2)。
- [ ] 3.6 集成测试:mock 作答 approve/deny/grant-hit/二次失败不升级四路。

验证:同 PR1;后台路径确认维持指引零改动。

## PR4 — 前端配置面 + 债清

- [ ] 4.1 wire:`update_project_sandbox_policy` daemon route + Tauri command
      (additive;白名单校验 policy 值)。
- [ ] 4.2 项目编辑面三态选择(定位现有项目设置 UI;radio/segmented + 一句话
      说明:放行=无沙盒 / 读写=默认 / 只读=硬隔离)+ vitest。
- [ ] 4.3 SBX-002:store raw/effective 分离 + GeneralTab 默认项固定 chip +
      注释修正 + vitest(「移除后复活」回归)。
- [ ] 4.4 全量:`cargo test -p everlasting --lib` + `--test e2e` + `pnpm test`
      + `pnpm build` + vue-tsc + clippy/fmt。
- [ ] 4.5 live:`scripts/turn-smoke.sh --sandbox-probe`(真实 LLM ReadOnly 命令
      无误杀 + 审计行含 face)+ 手动一轮 Edit 读写档 cargo test 冒烟。

## 收尾(全 PR 后)

- [ ] spec:`sandbox-executor.md` 增补三态契约 / resolve_policy 求值序 / 升级
      闭环 / Plan 回退;ROADMAP A2+ P3 行移档;DEBT.md 销 SBX-002/003/004。
- [ ] journal 记录;非目标(follow-up)挂账:后台升级闭环、bwrap、网络白名单。

## 风险文件与回滚点

- 高风险:`permissions/check/permission.rs`(短路点)、`sandbox/mod.rs`(gate
  重接)、`agent/chat_loop/drive.rs`(tool list)。每 PR 原子提交,回滚 =
  revert 单 PR;全局行为回滚 = kill-switch 关(既有通道)。
- `filter_tools_for_mode` 签名变化是唯一跨模块签名改动(调用点单一:drive.rs)。
