# Backend 开发规范

> 本目录沉淀 backend(`app/src-tauri/src/`)的代码规范 — Rust 模块组织、SSE / Provider 实现、agent loop、workflow / permission / skill / memory / subagent 等。

## 规范索引

| 指南 | 描述 | 状态 |
|------|------|------|
| _(empty — 待 workflow 沉淀时填充)_ | | |

## 何时读本目录

- implement 入口 → `wf-before-dev` skill → `list_dir .everlasting/spec/`
- check → 对照本目录的规范做合规检查
- done → 用 `wf-update-spec` skill 把本次决策 / 坑 / 新 pattern 写进来

## 沉淀方向(指引,非硬性)

- **LLM contract**:Anthropic / OpenAI Provider trait + SSE 解析 + Extended Thinking + wire shape 的一致性
- **Agent loop**:`run_chat_loop` 签名演进 + skip_persist gate + worker 嵌套
- **Permission**:`⑨ 关 5-tier` 决策层 + Mode `edit` / `plan` / `yolo` + 审计日志
- **Memory / skill**:`SkillSource::Plugin` + `SkillSource::Builtin` 双源 + `wf-*` skill loader
- **Workflow**:engine / content 分离 + `set_task_state` hook + spec distillation
- **Subagent**:dispatch turn 注入 delegation template + role gate

## 模板参考

参见 `.trellis/spec/backend/index.md` 的样式(状态表 + how to fill)。本目录的沉淀自由 markdown,不强制结构;但建议保持 `标题 + 场景 + 规范 + 反例` 四段式,便于后续 agent 检索引用。