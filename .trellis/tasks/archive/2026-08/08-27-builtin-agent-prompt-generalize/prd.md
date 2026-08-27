# 内置 dev 插件提示词通用化:脱 cargo/pnpm 硬编码 + 项目层覆盖同步

2026-08-27 调查确认:everlasting 作为**通用** agent 工具,其内置 dev workflow 插件的子代理提示词却硬编码了 Rust/前端栈命令。这些内容经 `include_str!` 编译进二进制发给所有用户——用户项目是 Python/Go/Java 时,checker 会拿着"不许把 `cargo test --lib` 缩小范围"这种纪律无处落地,甚至误导模型在本仓库外的项目里乱跑 cargo。

## 层结构(为什么分两层修)

- **builtin 层**:`app/src-tauri/resources/builtin-workflow/`,编译期嵌入(`agent/workflow/builtin.rs`),loader 查不到项目层时 fallback 到这里 → **必须栈中立**(发给所有用户)。
- **项目层**:`.everlasting/workflow/<name>/`,项目自己的覆盖层,优先生效;但按 `backend/workflow-plugin-builtin.md`,它是 builtin 的 **byte-identical 人工镜像**,不是自由定制的第二份内容——所以本任务把通用化做在 builtin,镜像整体重灌(2026-08-27 审查后统一口径:初稿"项目层保留 Rust 特化命令"与 spec 镜像约定冲突,已废弃)。

## 证据清单(builtin 层,全部需改)

### `dev/agents/checker.md`

- L22-24 工作流第 2 步命令式列出 `cargo test --lib` / `cargo clippy --lib --tests -- -D warnings` / `pnpm test`
- L35-36、L46-47 PASS/FAIL 报告模板字段字面叫 `cargo test:` / `cargo clippy:`
- L55 可用工具注解"(跑 cargo / pnpm)"
- L63 关键纪律"`cargo test --lib` 不能改成 `cargo test <specific>`"(跨栈语境下无意义且误导)
- L26 残留引用 **`.trellis/spec/`**(另一工具的目录!同文件 L27 与项目层副本均写 `.everlasting/spec/`)→ 统一为 `.everlasting/spec/`

### `dev/agents/implementer.md`

- L15 目标句"改完 `cargo check` + `cargo test --lib` 确认不破坏现有行为"
- L22 第 4 步跑检查全 cargo(`cargo check --tests` / `cargo test --lib` / `cargo clippy`)
- L27 报告字段 "**Test results**: cargo test 通过情况"
- L33 "(跑 cargo / pnpm 测试)"
- L43 "不要重复跑 cargo build(R2:复用 cargo cache)"

### `dev/workflow.json`(delegation_templates)

- L30 implementer 模板内嵌"改完 cargo check + cargo test --lib 确认不破坏现有行为"
- L31 checker 模板内嵌"请跑 cargo test --lib + cargo clippy + 检查 spec 合规"

### 其他 `.trellis` 残留(builtin 层,grep 全量确认共 3 处)

- `dev/skills/wf-update-spec/SKILL.md:16`:"借鉴 `.trellis/spec/` 结构,但物理独立(见 Q7)"——把设计出处写在发给所有用户的提示词里,Q7 无处可查。
- `review/skills/wf-overview/SKILL.md:11`:"**别去读 `.trellis/workflow.md`**——那是 Trellis 框架给人看的…"——这是本仓库 dogfood 特有的纠偏提示,普通用户根本没有 `.trellis/` 目录,纯属噪音;同类"应用面狭隘"问题。

## 项目层镜像漂移(spec 约定未落实的实证)

`backend/workflow-plugin-builtin.md` 规定:**source of truth = builtin 源目录**,`.everlasting/workflow/<plugin>/` 必须是 **byte-identical 镜像**(人工同步,用 `diff -r` 验证)。当前实测(`diff -r`):

- `dev/workflow.json` 两层一致(states = planning/in_progress/done)✅;
- 但 agents/skills 六个文件停留在**旧状态词汇表**(implement/check,builtin 已合并为 in_progress):checker.md、implementer.md、researcher.md、wf-before-dev/SKILL.md、wf-check/SKILL.md 各处 State 措辞不一致,wf-check 的"不通过怎么办"分支两边语义已分叉;
- checker 的 `.trellis/spec/` vs `.everlasting/spec/` 差异也是漂移的一部分;
- 另:`README.md` 仅存在于 builtin 侧(diff -r 会报 Only in)。README 不参与加载,是否纳入镜像规则留给实现时判断(倾向 builtin 保留、验收时排除该文件)。

## 已核实无需改动

- `registry.rs` 的 `builtin_subagents()`(researcher / general-purpose):栈中立。
- review 插件 `reviewer.md`:栈中立。
- 参照范式:同插件 `dev/skills/wf-check/SKILL.md` L10-11 已是正确写法——"lint(cargo clippy / eslint 等,**按项目**)""typecheck(cargo check / tsc)"。agents / delegation_templates 的通用化照此风格展开即可,不引入新机制。

## 方案(A 采用;B 为未来增强,不在本任务)

**A(采用):提示词内通用探测,零新机制。**
让 checker / implementer 自行推断项目的验证命令,顺序:
1. 项目文档优先(`AGENTS.md` / `CLAUDE.md` 里记载的 lint/typecheck/test 命令);
2. 清单文件推断:`Cargo.toml`(含 workspace 时注意 default-members 陷阱)、`package.json`(+包管理器 lockfile 推断 pnpm/yarn/npm)、`pyproject.toml`、`go.mod`、`pom.xml`/`build.gradle.*`;
3. 都没有 → **不阻塞、不冒猜**(2026-08-27 审查后按角色写死,消掉"或"字余地):checker 走最小验证(按本次改动文件类型挑能跑的检查)并在 verdict 里注明"未找到全量套件命令,以下为最小验证";implementer 同理,并在返回 summary 的 Known issues / Open questions 中列出。注意 checker.md 与 implementer.md 的约束清单都**没有声明 ask 类工具**(checker 还只读、禁 dispatch),所以两个子代理都不要在提示词里许诺向用户提问的行为——需要向用户澄清时,一律由主 LLM 根据子代理报告发起。

报告模板字段改为 `<test cmd>: X passed, 0 failed` 这种占位形式;纪律抽象化:"不要缩测试范围(不得以单测代替项目全量套件)"、"lint 告警必须修,不得用 suppress(如 `#[allow]`/`eslint-disable`)绕过"、"避免重复冷构建,复用增量缓存"。

**B(未来增强)**:workflow.json 增加验证命令配置槽,{verify_cmds} 占位符由主 session 注入——需要新管道,另开任务。

## 项目层 `.everlasting/workflow/<plugin>/`:按 spec 落实镜像同步(dev + review)

按现行 spec 约定(byte-identical mirror),项目层**不保留**仓库特化命令——通用化后的 builtin 提示词会经由探测机制(本项目 `AGENTS.md` 已精确记载 cargo/pnpm 验证命令与 workspace 陷阱,探测指引第 1 优先级即项目文档)拿到正确命令。若日后想要"每仓库自定义验证命令槽",归入方案 B 另立任务。

**dev 漂移**:agents/skills 六文件停留旧状态词汇表(implement/check)+ spec 路径分叉,见上节;builtin 改完后以 builtin 为准整体重灌。

> 2026-08-27 外部审查补充:**review 也有漂移**,diff -r 实测两处,一并纳入本任务:
>
> - `review/skills/wf-overview/SKILL.md:11` —— 项目层已改为通用表述("不要手动去读状态机配置"),builtin 侧仍带 `.trellis/workflow.md` dogfood 纠偏(即 R2 残留之一)。R2 重写 builtin 时**照项目层措辞写**,改完漂移自然消失。
> - `review/skills/wf-review-method/SKILL.md:26` —— 项目层比 builtin 多一段实证教训:上一轮评审(session 6b313ce4)两个 reviewer 都漏传 `model` 参数、静默继承父默认模型,"多模型分歧"价值落空,事后从 DB 才发现。这是违反镜像约定的单方面领先改动,**决策:收编进 builtin**——教训本身对普通用户普适(model 强制必传,漏传静默继承),但需去掉 dogfood 细节(session id / DB `model_display` 回填经过不进 builtin 提示词);收编后项目层回归 byte-identical。

落地动作:builtin 改完后,**以 builtin 为准整体重灌 dev + review 两份镜像**,并跑 `diff -r --exclude=README.md`(两个 plugin)作为验收。

## 同步约束(def.rs 镜像)

`src/agent/workflow/def.rs` 的 `default_workflow()`(L420-433 delegation_templates)与 `resources/builtin-workflow/dev/workflow.json` 当前内容逐字一致(2026-08-27 人工比对属实),但这层等价关系**目前没有任何测试兜底**:builtin.rs 的测试只做 JSON parse + validate() + frontmatter 检查(`builtin_dev_workflow_json_validates` 等),workflow/mod.rs 只有 `validate_passes_on_default_workflow`,"逐字等价"仅存在于 builtin.rs L13 的注释声明。改 JSON 必须同步改 Rust 常量,否则 fallback 与文档分裂。

> 2026-08-27 外部审查修正:PRD 初稿误称"有测试断言两边一致",实际不存在——本任务正好两边都要动,**顺手补一个等价性测试**(反序列化 `BUILTIN_DEV_WORKFLOW_JSON` 后与 `default_workflow()` 断言相等;若 `WorkflowDef` 未实现 PartialEq 则至少对 delegation_templates / breadcrumb 等文本字段做键级比对;review 插件无 Rust 镜像常量,不受此约束,勿画蛇添足)。

## Requirements

- R1: builtin 层脱栈通用化(`dev/agents/checker.md`、`dev/agents/implementer.md`、`dev/workflow.json` delegation_templates),语义保留(只读角色边界、update_checklist 纪律、PASS/FAIL 判定结构不变)。
- R2: builtin 层 `.trellis` 引用清零(grep 确认 3 处:dev/checker.md L26 改指 `.everlasting/spec/`;dev/wf-update-spec SKILL.md L16 与 review/wf-overview SKILL.md L11 重写为通用表述——后者照项目层现成措辞)。
- R3: `workflow/def.rs` 的 `default_workflow()` delegation_templates 逐字同步,并**新增等价性测试**(反序列化 BUILTIN_DEV_WORKFLOW_JSON == default_workflow(),字段级亦可——当前无任何测试守护这层等价,见"同步约束"节)。
- R4: 项目层 `.everlasting/workflow/` 以 builtin 为准整体重灌**两份**镜像(dev + review,byte-identical):dev 消灭旧状态词汇表漂移;review 的 wf-review-method 先把 6b313ce4 教训去 dogfood 化收编进 builtin 再统一。
- R5: 两条逐字不变式改由机制守护:builtin workflow.json ↔ def.rs 镜像(R3 新增测试);builtin ↔ 项目层镜像(diff -r --exclude=README.md 验收命令)。
- R6: `resources/builtin-workflow/dev/README.md` 措辞修正:"二者内容需保持同步"改为明确同步范围 = workflow.json + agents/ + skills/,README 自身不参与镜像、不进 byte-identical 约定,消除验收歧义(2026-08-27 审查定案,不再留给实现时)。

## Acceptance Criteria

- [ ] `grep -rn "cargo\|pnpm" app/src-tauri/resources/builtin-workflow/dev` 仅剩 wf-\* skill 中"按项目举例"式提及(wf-check 现有风格),agents/\*.md 与 workflow.json 零栈硬编码。
- [ ] `grep -rn "\.trellis" app/src-tauri/resources/builtin-workflow/` 零命中。
- [ ] checker/implementer 含探测指引(项目 AGENTS.md → 清单文件 → 无法判定则最小验证 + 报告标注,澄清落回主 LLM),报告模板用 `<test cmd>:` 占位。
- [ ] `diff -r --exclude=README.md` 分别跑 dev / review 两对目录,builtin ↔ 项目层零差异。
- [ ] R3 的等价性测试落地并通过(`cargo test -p everlasting --lib workflow`,从根 workspace 跑或 cd app/src-tauri均可)。
- [ ] spec 里的验证清单过一遍:`cd app/src-tauri && cargo build --lib`、`cargo test --lib builtin`、`cargo test --lib list_plugins`、`cargo test --lib loader`、`cargo clippy --lib --tests -- -D warnings`(WSL 记得 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`);另跑 `-p everlasting --lib` 全量确认无回归。
- [ ] builtin.rs 自校验测试通过(include_str! 内容仍有合法 frontmatter)。

## Notes

- 纯提示词文案 + JSON/Rust 常量同步,不改代码逻辑与 dispatch 行为。
- 改完建议跑一轮 app 内实测(建临时 session 走一次 in_progress → 派 implementer/checker)看子代理行为不劣化,可用 `scripts/turn-smoke.sh` 风格验证链路。
