# ROADMAP — 技术路线图

> **维护承诺(2026-06-10 锁定)**:本文档是 living document,随功能完善 / 需求更改及时更新。**实施 / git log 是终极归档**(完整 commit 列表见 `git log --oneline -20`),本文档只列宏观计划。
>
> 文档职责:
> - ✅ 做什么 + 什么时候做(V2 4 档分类 + 已实施粗粒度归类)
> - ❌ **不**讲具体实现细节(实现走 [IMPLEMENTATION.md §1](./IMPLEMENTATION.md) / [ARCHITECTURE.md](./ARCHITECTURE.md))
> - ❌ **不**讲历史决策(决策走 [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志))
>
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),实现讲解 + 决策日志见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 已实施(MVP 主体 + 路线图外完成)

> 粗粒度归类,不逐 commit 罗列。具体 commit 走 `git log --oneline -20`。

### 1.1 MVP 主体(原 7 步路线图)

| 阶段    | 里程碑 | 状态 |
|---------|--------|------|
| MVP     | 步骤 1:Tauri 2 + Vue 3 + Rust 骨架,LLM 流式对话 | ✅ |
| MVP     | 步骤 2:Tool Calling(`read_file` / `write_file` / `shell`)+ Agent Loop | ✅ |
| MVP     | 步骤 3a:SQLite 持久化 + Session 管理 | ✅ |
| MVP     | 步骤 3b-1:Project 数据模型 + 顶部 Tabs UI | ✅ |
| MVP     | 步骤 4:Git 集成(worktree + opt-in attach / detach / delete) | ✅ |
| MVP     | 步骤 5:WSL 体验(spike-001 验证) | ✅ |
| v1      | 步骤 6a:多 Provider(Anthropic / OpenAI,自研 Provider trait) | ✅ |
| 跨阶段  | 步骤 8:代码重构(5 PR — lib.rs / db.rs / 前端 sub-components / 文档 / STRUCTURE.md) | ✅ |

> 步骤 3b-2(完整三栏 UI + rig-core 迁移)已废弃,详见 [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION.md#4-决策日志)。

### 1.2 路线图外完成

- ✅ Anthropic extended thinking 块展示 + 持久化
- ✅ spike-005 follow-up 7 PR(UI 紧凑 header / git_branch 显示 / 启动 batch backfill / pwd `~/` 简化 / write_file tracing / LLM cancel 机制 / markdown 渲染)
- ✅ 字体栈调整(HarmonyOS Sans SC 子集打包)
- ✅ 6 UI/状态 bug 修复(streamController 架构 + 顶栏窗口控制 + Markdown 表格 + Tauri 2 权限 + minimize icon + 顶栏 padding)
- ✅ 工具集扩展批次(`edit_file` / `grep` / `glob` / `list_dir` + ReadGuard + Bash 落盘 + cat -n 行号)
- ✅ provider catalog hot-reload + display_name optional + session model_id binding(2026-06-10)

---

## 2. V2 路线图分类(2026-06-10 重排)

### 🟢 第一档 — 立刻做(4 项)

| 编号 | 功能 | 价值 |
|------|------|------|
| A4   | Token 用量统计 | 成本可见性 + 优化依据 |
| B5   | Memory(user + project,2 层先做) | 跨 session 指令一致性 |
| C1   | 取消机制完整化 | 长跑任务用户控制权 |
| D1   | session 重命名 / 标记 | session 列表可读性 |

### 🟡 第二档 — 接着做(7 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| A2 + B7 | **权限系统 + 多模式(合并工作组)** | ⑨ 权限检查 + ⑧a Mode 联动 |
| B3   | /command 命令面板 | 输入层扩展 |
| C3   | Context 压缩 + token 硬卡 | ⑤ context 构造的 token 预算 |
| C4   | 审计日志 | ⑨ ⑩ ⑬ ⑮ 事件可回看 |
| B2   | @文件补全 | 输入层扩展 |
| D2   | SQLite FTS5 全局搜索 | 历史消息可检索 |
| D3   | session 内消息编辑 / 重发 | session 灵活交互 |

### 🟠 第三档 — 缓做(8 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| B6   | Subagent(main agent 派 worker agent,独立 context,summary 回填) | **harness 学习价值高**,依赖 B5 Memory |
| B4   | Skill 系统 | 指令层扩展 |
| B9   | 生成式 UI(4 primitives — button / selector / diff / code_block) | 输出层扩展 |
| C2   | 循环检测 | ⑬ 关卡实现 |
| C6   | 大输出截断统一 | ⑩ ⑫ 边界处统一处理 |
| B1   | 图片支持(multimodal) | 输入层扩展 |
| A5/A6 | 错误处理完善 + README + demo | 打磨 |
| A7   | RDP 双屏 position bug 修复 | 已知 issue 收尾 |

### 🔴 第四档 — 最远远期(app 主体完善之后)(3 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| B8   | 可编排(DAG workflow) | 编排层,多 agent 串行/并行 |
| B10  | 飞书 IM | **触发 daemon 化**,重大架构变更 |
| B11  | 云端同步(Cloudflare Workers + D1) | 个人远程遥控通道 |

---

## 3. 移除项 / 已废弃(V2 重排,2026-06-10 决定)

> **不再做**的项目归这里,避免认知噪音。决策日志已覆盖"为什么不做"。

### 3.1 移除(明确不做)

| 编号 | 项目 | 一句话原因 |
|------|------|------------|
| A1   | xterm.js 嵌入式终端 | v1 `shell` tool + 30K 落盘已覆盖"看 agent 在跑啥"的需求 |
| A3   | MCP 暴露 | 个人工具,工具集对外开放是 Claude Code 生态已经解决的问题,本项目杠杆不足 |
| C5   | Provider 限流(令牌桶) | 个人使用场景未撞到限流;v1 之后看实际用量再评估 |

### 3.2 已废弃(历史决策,保留归档)

- **3b-2 完整三栏 UI + rig-core 迁移** — rig-core 0.38.1 弃用(2026-06-09 决策,自研 `Provider` trait 已完整支持多 Provider),3b-2 同步废弃
- 决策依据见 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志)对应日期条目

---

## 4. 关键理解纠正(必须留笔,2026-06-10)

### 4.1 B6 = Subagent(**不是**用户切角色)

- **正确语义**:main agent 在 ⑥ LLM 决策后,派出一个 **worker agent** 跑独立 context(独立 messages / 独立 token 预算),完成后由 worker 把 **summary** 回填给 main agent
- **类比**:Claude Code 的 Task tool / OpenHands 的 subagent
- **harness engineering 学习价值高**:消息流隔离、context 预算管理、summary 注入位置,都是 harness 设计的核心命题
- **依赖**:B5 Memory 落地后(worker 需要 user/project memory 上下文)再做,效果最佳

### 4.2 B7 = Mode 是 A2 权限系统的 UX 层

- **正确语义**:B7(mode = chat / plan / review / background / yolo)**不是**独立功能,是 A2 权限系统的**前端 UX 层**
- **联动链**:前端 mode 切换 → 后端 ARCHITECTURE §2.2 **⑧a Mode 检查**(plan 模式拒 tool_use / review 模式只读 / yolo 跳过 ⑨) + ⑨ 权限检查 联动
- **工作组划分**:A2 + B7 合并做(基础设施 + UX 一组),列在第二档

### 4.3 A2 + B7 合并工作组(第二档 7 项之一)

- A2(后端 ⑨ 权限基础架构) + B7(前端 mode 切换 UI)是一组工作,不能拆
- 实施顺序:先 A2 后 B7(B7 依赖 A2 暴露的 mode 配置)

---

## 5. 后续维护承诺

- **本文件改动时机**:
  - 完成 V2 任何一档任何一项 → 移到 §1 已实施 + 加 commit hash 引用
  - 重新审视 V2 档位(升档 / 降档 / 移除) → 直接编辑 §2 / §3 + 在 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 追加 ADR 条目
  - V2 → V3 重排 → 整体替换本文件或归档到 `docs/_archive/`
- **不做的边界**:
  - 不在本文件列具体 commit / PR 编号
  - 不在本文件做技术细节(具体设计走 BACKLOG.md / 各 spec 文件)
  - 不在本文件做决策追溯(走 IMPLEMENTATION §4 决策日志)
- **其他文件引用本文件的统一形式**:`[docs/ROADMAP.md §X](./ROADMAP.md#X)`,不复制路线图内容到其他文件
