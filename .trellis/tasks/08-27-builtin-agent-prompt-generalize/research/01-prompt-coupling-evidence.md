# Research: builtin 提示词栈耦合与镜像漂移 — 原始证据

- Query: 2026-08-27 调查内置 dev/review workflow 插件提示词的栈硬编码、`.trellis` 残留、项目层镜像漂移
- Scope: internal
- Date: 2026-08-27
- 说明:结论与需求已固化进 prd.md,本文件只存**原始命令输出**,供 implement/check 复核,不必重新 grep。

## 1. cargo/pnpm 硬编码(builtin 层)

```
$ grep -rn "cargo\|pnpm" app/src-tauri/resources/builtin-workflow/
dev/skills/wf-check/SKILL.md:10: - lint(cargo clippy / eslint 等,按项目)      ← 正确范式
dev/skills/wf-check/SKILL.md:11: - typecheck(cargo check / tsc)                ← 正确范式
dev/agents/checker.md:22/23/24/35/36/46/47/55/63                                ← 需改
dev/agents/implementer.md:15/22/27/33/43                                        ← 需改
dev/workflow.json:30/31 (delegation_templates)                                  ← 需改
```

## 2. `.trellis` 残留(builtin 层,共 3 处)

```
$ grep -rn "\.trellis" app/src-tauri/resources/builtin-workflow/
dev/skills/wf-update-spec/SKILL.md:16:- 借鉴 `.trellis/spec/` 结构,但物理独立(见 Q7)
dev/agents/checker.md:26:   - 改的代码路径是否在 `.trellis/spec/` 里有 guideline?有的话对照检查
review/skills/wf-overview/SKILL.md:11:> ⚠️ **别去读 `.trellis/workflow.md`** —— ...(dogfood 专属纠偏)
```

## 3. dev 镜像漂移(`diff -r` 2026-08-27 实测)

- `Only in builtin: README.md`
- `workflow.json` 两层一致(states = planning/in_progress/done)
- agents/skills 六文件停留旧状态词汇表:`State: check`(checker)、`State: implement`(implementer)、
  "进 implement 状态"(researcher L43)、"implement 入口"(wf-before-dev)、"implement 每项后 / check"(wf-check)、
  wf-update-spec 项目层少一行(即 §2 的 `.trellis` 行)
- checker spec 路径分叉:builtin `.trellis/spec/` vs 项目层 `.everlasting/spec/`

## 4. review 镜像漂移(`diff -r` 2026-08-27 实测,仅两处)

- `wf-overview/SKILL.md:11`:项目层已是通用表述("不要手动去读状态机配置"),builtin 带 `.trellis/workflow.md` 纠偏
- `wf-review-method/SKILL.md:26`:项目层单方面领先一段 —— session 6b313ce4 实证(两个 reviewer 均漏传 `model`,
  静默继承父默认模型,"多模型分歧"落空,事后从 DB `model_display` NULL 发现并回填)。决策(prd R4):去 dogfood 化收编进 builtin。

## 5. def.rs ↔ workflow.json 等价性无测试守护(外部审查 #1 的复核)

```
$ grep -rn "BUILTIN_DEV_WORKFLOW_JSON|default_workflow()" src --include="*.rs"
# 仅三类引用:builtin.rs parse+validate 测试;mod.rs:509 validate_passes_on_default_workflow;
# 各处 fallback 注释。没有任何 assert 比较 JSON(反序列化后或原文)与 default_workflow()。
# "逐字等价"仅是 builtin.rs:13 的注释声明。当前一致系人工比对属实(def.rs:420-433 vs workflow.json:28-32)。
```

## 6. 已核实无需改动

- `agent/subagent/registry.rs::builtin_subagents()`(researcher / general-purpose):栈中立 ✅
- `review/agents/reviewer.md`:栈中立 ✅

## Caveats / Not Found

- 子代理工具集(checker/implementer 约束清单)均未声明 ask 类工具;"向用户澄清由主 LLM 发起"是据此推断的安全边界,
  未逐一追 tool registry 运行时白名单。若实现时发现子代理实际可拿到 ask 工具,R1 文案可相应放宽,但默认按 PRD 写死。
