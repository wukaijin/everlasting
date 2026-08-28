## 7. 第二个 plugin `review`(评审流)—— 已落地

见 §4.2。**2026-07 起落地**:builtin review plugin(`resources/builtin-workflow/review/` + `BUILTIN_REVIEW_WORKFLOW_JSON` + 4 个 review skills(wf-overview/wf-review-prep/wf-review-method/wf-synthesize))+ `commands/review.rs` + `get_review_state` IPC 双暴露、前端矩阵视图(review-state.json 三态,07-26)、epic 前置基建(07-28 subagent resume C1 + TaskStatus 自定义 plugin state C0)。**08-26/27 review 内置化完成**(workflow-plugin-builtin spec:builtin 源为 source of truth,项目层 byte-identical 镜像)。回合制 A / 实时群聊 B 的取舍仍按 §4.2 记录,群聊路径已随 07-29 group chat 逐步验证。

> ❓ **Q8(延迟)**:评审流走 A 还是 B,何时立项 —— A(回合制文件态矩阵)已事实落地,见 [ROADMAP §1.2 C2 review-viz 行](../../ROADMAP.md#12-路线图外完成);B(实时群聊)随 08-27 群聊路径持续验证。

---
