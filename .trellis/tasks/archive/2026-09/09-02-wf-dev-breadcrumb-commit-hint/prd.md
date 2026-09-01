# wf-dev breadcrumb 加 commit 指引

## Goal

wf dev 内置工作流在"提示层"对 commit 完全缺位:三态 breadcrumb(每轮注入请求尾部的状态提示)没有一句提到提交工作改动;引擎层唯一的拦截是状态门禁(`requires_user_confirm`),管的只是 transition 本身。结果:走 wf dev 的会话在收尾时没有任何机制引导"把代码改动提交掉",对比 Trellis 工作流(breadcrumb 明确写 `commit (Phase 3.4) -> /finish-work`,且 `/finish-work` 拒绝 dirty tree)少了对应物。

本任务在 wf dev 的 `in_progress` / `done` breadcrumb 文案中补上 commit 引导,使流程收尾自然覆盖"工作改动落 commit"这一步。

## Requirements

1. 改动范围(三处,内容保持一致):
   - `app/src-tauri/resources/builtin-workflow/dev/workflow.json` 的 `breadcrumb.in_progress` / `breadcrumb.done`(**builtin 源,source of truth**)
   - `app/src-tauri/src/agent/workflow/def.rs` `default_workflow()` 常量中对应两条 breadcrumb 字符串(`builtin_dev_json_equals_default_workflow_constant` 单测守护 JSON ↔ Rust 等价,漏改会被拦,但必须同步改对)
   - `.everlasting/workflow/dev/workflow.json`(byte-identical 项目镜像)
2. 文案要求:
   - `in_progress`:收尾时把任务代码改动整理成逻辑 commit 并提交,然后再请用户确认转 done。
   - `done`:归档前确认工作改动已 commit,未提交则先提醒用户。
   - **栈中立**:只写 git 通用动作,不得出现本仓库特化命令(cargo/pnpm 等)——遵守 `workflow-plugin-builtin.md` §提示词内容约定。
   - **无 dogfood 泄漏**:不得出现 commit hash、内部引用。
   - 中文,与既有文案风格/密度一致,尽量不显著加长。
3. `planning` breadcrumb 不动(commit 与调研阶段无关)。

## Acceptance Criteria

- [ ] 三处文件中 `in_progress` / `done` breadcrumb 均含 commit 引导文案,内容一致
- [ ] `cargo test -p everlasting --lib builtin` 全绿(含 JSON ↔ Rust 等价性单测与 builtin 3 件套)
- [ ] `diff -r --exclude=README.md app/src-tauri/resources/builtin-workflow/dev .everlasting/workflow/dev` 无差异
- [ ] `grep -rn "Wf · in_progress\|Wf · done" app/src-tauri/src` 无遗留旧文案(Rust 测试锚定旧字面量的需连带更新,若发现)
- [ ] 无栈特化命令、无 dogfood 信息进入新文案

## Out of Scope

- 不改 app 权限引擎的 shell 分类(Edit 模式放行 `git commit` 是既有设计)
- 不改 lefthook、不改 Trellis 工作流
- 不改 review 插件
