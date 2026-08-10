## 3. 项目能力边界(本功能)

### 3.1 做(engine + 第一个 plugin 范围内)

1. **workflow engine**(Rust 固定):session 级 workflow 开关 + 注入 seam + state 门控 + state 转移 + plugin 加载/切换
2. **第一个 plugin `dev`(开发流)**(MVP):planning/implement/check/done 四态 + researcher/implementer/checker 三角色 + 沉淀闭环
3. **plugin 内容文件态**:`.everlasting/workflow/<name>/`(state 定义 + breadcrumb 模板 + 角色映射);项目可改,覆盖 builtin 默认
4. **task 文件态记账**:agent 自动产 `.everlasting/tasks/<slug>/`;**无 task DB 表、无 task UI、用户不操作 task**
5. **wf-* skill 包**:作为 plugin 默认内容,走 B4 三层覆盖
6. **沉淀闭环**:task done 时把决策/教训升级进 `.everlasting/spec/`
7. **任务交接**:task 跨 session 靠文件位置天然达成
8. **UI workflow 切换**:session 开 workflow 时选 plugin;会话中可切

### 3.2 不做(硬约束)

| 不做 | 原因 |
|---|---|
| ❌ **任何 task UI**(picker/list/board/切换器) | task 是 agent 记账副产物,用户不操作 |
| ❌ **task DB 表 / session↔task 关联** | task 纯文件态;session 不绑 task |
| ❌ **B8 DAG 拓扑** | 留 B8(第四档);engine 不做 task 间编排 |
| ❌ **强制全局 workflow** | opt-in;不开 = 现有行为零改动 |
| ❌ **强制全局 TDD** | per-checklist-item opt-in |
| ❌ **workflow 绑定 Mode** | Mode 是权限旋钮,workflow 是流程旋钮,正交;state 不替用户切 Mode |
| ❌ **新建 session 强制选 task** | session 创建不被 task 打断 |
| ❌ **重写 agent core** | 复用 [`run_chat_loop`](../../.trellis/spec/backend/agent-loop-architecture.md) 26 参;走注入 seam + tool 层 |
| ❌ **第二个 plugin `review`(评审流)+ 新通讯架构**(本阶段) | 作为愿景写入 §7,延迟讨论 |

---
