# A2+ P3c 沙盒 UX 增强档:三态 per-project 配置 + Plan 模式沙盒化 + 失败升级闭环

> 前置:P3b(`08-31-a2-p3b-sandbox-executor`,已归档)交付 Landlock+seccomp 执行器,
> 触发面 = 仅 ReadOnly 档。源方案:
> [docs/_history/2026-08-28-a2-shell-classification.md](../../../docs/_history/2026-08-28-a2-shell-classification.md) §4。
> spec 基线:[sandbox-executor.md](../../../.trellis/spec/backend/sandbox-executor.md)。

## 背景与问题

P3b 的触发面窄:只有 `classify_prefix` 判为 `ReadOnly` 的命令进沙盒,配置面只有
全局 kill-switch(`sandbox_enabled`)+ 全局额外可写目录(`sandbox_extra_writable`)。
两个缺口:

1. **SideEffect 档静默且无界**:`SIDE_EFFECT_WHITELIST`(cargo/pnpm/rustup/gh/…)
   在 Edit 模式静默放行且**不进沙盒**——白名单的信任假设是「项目内可恢复」,但
   `rustup update` 这类多态命令实际出网 + 写全局目录,完全在假设之外。
2. **Plan 模式无 shell**:`filter_tools_for_mode` 把 shell 工具族整个滤掉,Plan
   会话的调查能力被砍(每条调查命令都只能靠 LLM 自己读文件)。

## Goal

把 P3b 的「仅 ReadOnly 档触发」升级为 **per-project 三态沙盒策略**(默认读写 =
全命令进沙盒、项目内自由、边界外收紧);Plan 模式重新暴露 shell 工具族,以
session 级只读面提供「确定性只读」保证;Edit 下以「沙盒 + 面外行为升级审批」
取代 shell 预弹窗。顺带清三条 SBX P3 债。

## 已确认事实(代码勘察)

- **Plan 模式今天没有任何 shell**:`filter_tools_for_mode`(permissions/mode.rs:57)
  把 `shell` / `run_background_shell` 整个从 tool 列表滤掉。Tier 4(permission.rs:411)
  存在 Plan 分支(SideEffect→Ask、ReadOnly→silent Allow)但因工具不可达而休眠。
- **worktree 是沙盒可写根**(policy.rs:133):`build_spec` 的 writable_roots 以
  `ctx.worktree_path` 开头;只读面需要把 worktree 移出可写根(exec 面保留)。
- **失败指引今天是模型介导的**:sandboxed 命令 exit≠0 且 stderr 命中
  `Permission denied|Read-only file system` → 尾部追加 `write_block_guidance`
  文案,由模型决定后续;无自动升级,断网(`Operation not permitted`)不触发指引。
- **per-project 配置载体**:projects 表已有 `metadata` TEXT 列(现全空,无消费方);
  或加专列(载体取舍归 design.md)。
- **沙盒判定链**:`sandbox::decide`(mod.rs:222)读全局 `sandbox_enabled` +
  `sandbox_extra_writable`,与 `gate`(四项与:capability / ReadOnly 档 /
  mode≠Yolo / enabled)组合。
- **Ask 暂停基建现成**:Tier 4 `ask_path` / QuestionStore 即「工具暂停等用户作答」
  机制,失败后审批可复用同构机制。

## 决策记录

- **D1(scope)**:bwrap namespace 增强档与网络白名单/egress **裁出本期**,留
  ROADMAP 挂账按需另立。理由:bwrap 是外部二进制依赖(与零新增依赖铁律两个
  量级),其收口的 interop socket 残余面攻击成本高;网络白名单两条路线
  (egress 代理 / Landlock ABI v4)都与「不赌内核版本」冲突。
- **D2(三态语义,修正案)**:三态 = per-project 沙盒**面**选择(非「能否写」;
  初版「只读面进 Edit 默认叙事」的设计已被否——Edit 本义就是项目内自由读写):
  - **放行(off)**:该项目无沙盒,经典判定 + Tier 4(= P3b 前行为)。
  - **读写(readwrite,默认)**:全命令进沙盒;面 = worktree 可写 + /tmp + spill
    + extras,断网;shell 弹窗被沙盒取代;**面外**写 / 断网失败 → 升级 Ask,
    批后一次性不沙盒执行(prefix-grant 可记「本 session 总是允许」)。
  - **只读(readonly,罕见)**:硬隔离/审计第三方仓库场景;全命令进沙盒,
    worktree 亦不可写;同款升级闭环(worktree 写也过审批)。
  - 不变量:Yolo 永不沙盒;全局 kill-switch = master(关 = 全局无沙盒,优先于
    一切档位);Tier 1–3 硬拒(危险命令静默拒绝)不被任何档位取代——沙盒只
    接管 shell 的 SideEffect/Ask 审批层。
  - **默认档即行为变更**:今天默认「仅 ReadOnly 档沙盒」→ 新默认「全命令沙盒
    (读写面)」。SideEffect 从「静默+完全无界」变「静默+有界(项目内自由)」;
    Ask 档从「预弹窗」变「沙盒内先跑、面外行为才问」;项目内日常
    (build/test/commit)零新增弹窗;出网依赖与面外写多一跳升级(可 grant 记住)。
  - 源方案依据(§4):「可写层只覆盖项目目录 + tmp,联网默认禁」+ 交付物拆分
    「P3c(**读写沙盒** + UX)」——读写沙盒 = 触发面扩到全命令。
- **D3(Plan 语义)**:Plan = session 级只读面:
  - 重新暴露 shell 工具族(替换今天的 `filter_tools_for_mode` 过滤),全命令进
    只读面沙盒,**无升级出口**——写失败只给模式感知指引(给 diff 提案 / 请
    用户切 Edit / 写 /tmp 合法逃生口,如 `CARGO_TARGET_DIR=/tmp/...` 调查型构建)。
  - Plan 的价值 = 「确定性只读」保证,不是「可批准的只读」;若提供升级出口则
    Plan 与 Edit+只读档无区别。mode system prompt 既有契约(「propose the
    change as a diff and ask them to switch to Edit mode」)对齐,不改。
  - 沙盒不可用(kill-switch 关 / 能力探测失败)→ 回退今天的工具过滤(shell
    不暴露);**不落到「Plan + 弹窗放行写」路径**,Tier 4 休眠 Plan 分支保持
    休眠。
- **D4(升级闭环机制)**:前台 `shell` 用**自动 Ask 卡**(工具检测到拦截特征 →
  经 QuestionStore 弹卡,批准 = 原命令一次性不沙盒重跑,拒绝 = 结果 + 指引回
  模型);后台 `run_background_shell` 本期**维持模型介导**(指引文案),后台
  升级闭环留 follow-up。理由:审批绑定确切命令文本(审计干净、防模型转述
  漂移)、基建同构(ask_path 复用)、happy path 省一轮 LLM 往返。已知代价
  (design 逐条处理):双重执行(升级仅因面外写/断网被拒触发,第一遍危险部分
  未发生);触发特征启发式保守匹配(漏报退化为指引,不放大误报)。
- **D5(配置载体,设计层裁定)**:见 design.md(projects 专列 vs metadata JSON,
  不阻塞产品语义)。

## Requirements

- **R1(三态配置)**:per-project 沙盒策略档 `off / readwrite / readonly`,
  默认 `readwrite`;写通道 daemon route + Tauri 双端 additive;前端项目设置面
  可改。优先级:全局 kill-switch(master)→ 项目档 → mode(Yolo 恒不沙盒)。
- **R2(触发面扩展)**:Edit + 读写/只读档下,**全命令**进沙盒(不再限于
  ReadOnly 档);Tier 4 的 shell 分支(SideEffect/Ask 弹窗)在沙盒档被短路,
  Tier 1–3 硬拒层照旧在其上方运行;判定层 `shell_trust` 语义零改动。
- **R3(只读面 spec 变体)**:`build_spec` 支持面选择——读写面 = 现状
  (worktree 可写);只读面 = worktree 移出可写根、保留 exec 面(项目脚本仍可
  运行);/tmp + spill + extras 两面均可写。
- **R4(Plan 沙盒化)**:Plan 模式 shell 工具族重新暴露,全命令只读面,无升级
  出口,写失败给模式感知指引;沙盒不可用时回退今天的工具过滤。
- **R5(升级闭环,前台)**:拦截特征(写:`Permission denied|Read-only file
  system`;网:`Operation not permitted`,仅 sandboxed 命令)→ QuestionStore
  Ask 卡(命令文本 + 拦截原因)→ 批准 = 原命令一次性不沙盒重跑 / 拒绝 = 结果
  回模型;prefix-grant 通道支持「本 session 总是允许」;后台路径维持指引文案。
- **R6(三条 P3 债)**:RULE-SBX-002(设置面 raw/effective 清单分离)、
  RULE-SBX-003(后台审计行挪 `registry.start` Ok 后)、RULE-SBX-004
  (`sandbox_enabled` 读挪进 gate 通过后)。
- **R7(可观测)**:审计记录面类型(readonly/readwrite);升级审批落审计
  (kind 复用或新增归 design);指引文案模式感知。

## 非目标

- Plan 模式升级出口(确定性只读是 Plan 的身份,D3)。
- 后台 `run_background_shell` 的自动升级闭环(follow-up)。
- bwrap namespace 增强档 / interop socket 残余面收口(D1)。
- 网络白名单 / egress 代理(D1;本期断网语义不变)。
- RULE-SBX-001(非 Linux 编译债;触发条件 = 转非 WSL 环境开发,不排期)。
- 判定层 `shell_trust` / Tier 1–3 语义改动。

## Acceptance Criteria

- [ ] AC1. Edit + 读写档(默认):集成测试——ReadOnly/SideEffect/Ask 三档代表
      命令均进沙盒;worktree 写成功;面外写(`~` 下)被拒;`/dev/tcp` EPERM;
      全程无预弹窗(Tier 4 shell 分支被短路)。
- [ ] AC2. Edit + 只读档:worktree 写被拒并触发升级 Ask 卡——mock 批准 → 原命令
      不沙盒重跑成功;mock 拒绝 → 失败结果 + 指引回模型;断网失败同样触发升级。
- [ ] AC3. Edit + 放行档:与 P3b 前行为一致——无 pre_exec,SideEffect/Ask 走
      Tier 4 原路径(Edit 下 SideEffect 静默 / Ask 弹窗)。
- [ ] AC4. Plan:`shell` / `run_background_shell` 回到 tool 列表;全命令只读面;
      写失败无 Ask 卡、有模式感知指引;Yolo 在任何档位下均不沙盒。
- [ ] AC5. 回退矩阵:全局 kill-switch 关 → 全档位无沙盒;能力探测失败 → Edit
      档 fail-open(现状行为)+ Plan 档回退工具过滤(不暴露 shell)。
- [ ] AC6. 升级闭环 grant:批准时选「总是允许」→ 同前缀后续命令不再弹卡直接
      不沙盒执行(复用既有 prefix-grant 存储,含 kind↔类别矩阵校验)。
- [ ] AC7. 配置面:项目设置三态读写生效(daemon + Tauri 双端);GeneralTab
      额外可写清单 raw/effective 分离,默认项(`~/.cargo`)不再出现「移除后
      复活」(RULE-SBX-002)。
- [ ] AC8. 审计:`SandboxedShellExecution` payload 含面类型;升级审批有审计行;
      后台审计行时序 = spawn 成功后(RULE-SBX-003);`decide` 的 config 读在
      gate 通过后(RULE-SBX-004,代码级复核)。
- [ ] AC9. 全量回归:`cargo test -p everlasting --lib` + e2e + `pnpm test` +
      build 绿;`turn-smoke.sh` live 过(含 `--sandbox-probe` 审计断言)。
- [ ] AC10. spec 收编:`sandbox-executor.md` 增补三态契约 / 升级闭环 / Plan
      语义;ROADMAP A2+ P3 行移档;DEBT.md 销 RULE-SBX-002/003/004。
