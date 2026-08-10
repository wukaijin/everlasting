#!/usr/bin/env python3
"""一次性执行 B1/B2 的机械链接修正(精确字符串替换,逐条报告)。

用法:python3 apply-fixes.py
输出:每条 (file, old, new) 的替换次数;0 次命中 = 需要人工检查。
"""
import sys

ROOT = "/usr/local/code/github/everlasting"

FIXES = [
    # ============ B1: decisions.md 索引去 IMPLEMENTATION/ 前缀 ============
    ("docs/IMPLEMENTATION/decisions.md", "./IMPLEMENTATION/decisions-2026-07.md", "./decisions-2026-07.md"),
    ("docs/IMPLEMENTATION/decisions.md", "./IMPLEMENTATION/decisions-2026-06.md", "./decisions-2026-06.md"),
    ("docs/IMPLEMENTATION/decisions.md", "./IMPLEMENTATION/decisions-2026-08.md", "./decisions-2026-08.md"),

    # ============ B1: decisions-2026-06.md 补 ../ ============
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "前置调研([docs/research/skill-system-survey.md](research/skill-system-survey.md)",
     "前置调研([docs/research/skill-system-survey.md](../research/skill-system-survey.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "新建 [`docs/ROADMAP.md`](./ROADMAP.md)",
     "新建 [`docs/ROADMAP.md`](../ROADMAP.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "(完整内容见 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排))",
     "(完整内容见 [ROADMAP.md §2](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排))"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "已归档到 [`docs/_archive/2026-06-04-roadmap-restructure.md`](_archive/2026-06-04-roadmap-restructure.md)",
     "已归档到 [`docs/_archive/2026-06-04-roadmap-restructure.md`](../_archive/2026-06-04-roadmap-restructure.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "沉淀在 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](_archive/2026-06-3b-1/FOLLOW-UP.md)",
     "沉淀在 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](../_archive/2026-06-3b-1/FOLLOW-UP.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](_archive/2026-06-3b-1/FOLLOW-UP.md)",
     "详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](../_archive/2026-06-3b-1/FOLLOW-UP.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "实施后归档,见 [ROADMAP §1.2 L3d 已实施条目](./ROADMAP.md#12-路线图外完成))",
     "实施后归档,见 [ROADMAP §1.2 L3d 已实施条目](../ROADMAP.md#12-路线图外完成))"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "完整设计见 [spike-007](./spikes/007-agent-autonomous-memory-plan.md)",
     "完整设计见 [spike-007](../spikes/007-agent-autonomous-memory-plan.md)"),
    ("docs/IMPLEMENTATION/decisions-2026-06.md",
     "设计文档沉淀为 [docs/INTERLEAVED-THINKING-DESIGN.md](./INTERLEAVED-THINKING-DESIGN.md)",
     "设计文档沉淀为 [docs/INTERLEAVED-THINKING-DESIGN.md](../INTERLEAVED-THINKING-DESIGN.md)"),

    # ============ B1: decisions-2026-07.md:13 补 ../ ============
    ("docs/IMPLEMENTATION/decisions-2026-07.md",
     "(孤儿 DB,详见 [DEBUG_DB §1.0](./DEBUG_DB.md#10-daemon-化后的三条解析路径2026-07-同步))",
     "(孤儿 DB,详见 [DEBUG_DB §1.0](../DEBUG_DB.md#10-daemon-化后的三条解析路径2026-07-同步))"),

    # ============ B1: 12-integration-points.md:52 死锚 ============
    ("docs/WORKFLOW-INTEGRATION/12-integration-points.md",
     "按 [问题 2 决定](#问题-2dev-workflow-有没有-templates-示例-skill-说明)",
     "按 [§14 Q2 决定](#14-待对齐汇总)"),

    # ============ B1: ARCHITECTURE 标题归一(恢复稳定锚点)+ 死锚修正 ============
    ("docs/ARCHITECTURE.md",
     "## 4. 决策:Agent Daemon 化(已实施,2026-07)\n\n**核心变更**",
     "## 4. 决策:Agent Daemon 化\n\n> **状态**:已实施(2026-07)。\n\n**核心变更**"),
    ("docs/ARCHITECTURE.md",
     "#### ⑨ Tool 权限检查(关键关卡,A2 + B7 落地,re-grill 2026-06-13,**已实施**)\n\n**5-tier 决策顺序**",
     "#### ⑨ Tool 权限检查\n\n> 关键关卡:A2 + B7 落地,re-grill 2026-06-13,**已实施**。\n\n**5-tier 决策顺序**"),
    ("docs/ARCHITECTURE.md",
     "(见 [§4](#4-决策agent-daemon-化为多-channel-接入铺路))",
     "(见 [§4](#4-决策agent-daemon-化))"),

    # ============ B1: ROADMAP §2 标题归一 + research 路径 + 锚点补齐 ============
    ("docs/ROADMAP.md",
     "## 2. V2 路线图分类(2026-06-10 重排,2026-06-13 收尾更新)\n\n### 🟢 第一档",
     "## 2. V2 路线图分类(2026-06-10 重排)\n\n> 2026-06-13 收尾更新。\n\n### 🟢 第一档"),
    ("docs/ROADMAP.md",
     "../research/at-file-injection-coding-agents-survey.md",
     "./research/at-file-injection-coding-agents-survey.md"),
    ("docs/ROADMAP.md",
     "../research/skill-system-survey.md",
     "./research/skill-system-survey.md"),
    ("docs/ROADMAP.md",
     "参见 [ARCHITECTURE §2.5.5](./ARCHITECTURE.md#255-⑤-context-超限降级)",
     "参见 [ARCHITECTURE §2.5.5](./ARCHITECTURE.md#255-⑤-context-超限降级c3-mvp2026-06-12-落地已实施)"),
    ("docs/ROADMAP.md",
     "架构见 [ARCHITECTURE §2.5.9](./ARCHITECTURE.md#259--并行-tool-执行l2-mvp2026-06-19-落地已实施)",
     "架构见 [ARCHITECTURE §2.5.9](./ARCHITECTURE.md#259-⑩-并行-tool-执行l2-mvp2026-06-19-落地已实施)"),
    # B5 extra: ROADMAP task 路径补 archive 前缀
    ("docs/ROADMAP.md",
     "主任务 `.trellis/tasks/07-08-workflow-integration/`(9 阶段 25 commit)",
     "主任务 `.trellis/tasks/archive/2026-07/07-08-workflow-integration/`(9 阶段 25 commit)"),
    ("docs/ROADMAP.md",
     "完整 PRD 走 `.trellis/tasks/07-13-b9plus-generative-ui-followup/`",
     "完整 PRD 走 `.trellis/tasks/archive/2026-07/07-13-b9plus-generative-ui-followup/`"),

    # ============ B1: BACKLOG §2 标题归一 + _archive 路径 ============
    ("docs/BACKLOG.md",
     "## 2. Agent Skill 系统 → 已落地 (B4 2026-06-18),详见 ROADMAP §1.2\n\n---",
     "## 2. Agent Skill 系统\n\n**状态**:已落地 (B4 2026-06-18),详见 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成)。\n\n---"),
    ("docs/BACKLOG.md",
     "../_archive/2026-06-3b-1/FOLLOW-UP.md",
     "./_archive/2026-06-3b-1/FOLLOW-UP.md"),
    ("docs/BACKLOG.md",
     "../_archive/backlog-appendix-A.md",
     "./_archive/backlog-appendix-A.md"),

    # ============ B1: CONTEXT 路径修复 ============
    ("docs/CONTEXT.md",
     "决策见 [IMPLEMENTATION §4 2026-06-18](../IMPLEMENTATION/decisions.md)",
     "决策见 [IMPLEMENTATION §4 2026-06-18](./IMPLEMENTATION/decisions.md)"),
    ("docs/CONTEXT.md",
     "完整列表见 [ARCHITECTURE §2.5.8](../ARCHITECTURE.md)",
     "完整列表见 [ARCHITECTURE §2.5.8](./ARCHITECTURE.md)"),
    ("docs/CONTEXT.md",
     "详见 [ARCHITECTURE §1/§4](../ARCHITECTURE.md)",
     "详见 [ARCHITECTURE §1/§4](./ARCHITECTURE.md)"),

    # ============ B1: DEBUG_DB 路径修复 ============
    ("docs/DEBUG_DB.md",
     "[`app/src-tauri/src/state.rs:304`](../../app/src-tauri/src/state.rs)",
     "[`app/src-tauri/src/state.rs:304`](../app/src-tauri/src/state.rs)"),
    ("docs/DEBUG_DB.md",
     "[`app/src-tauri/src/db/migrations.rs`](../../app/src-tauri/src/db/migrations.rs)",
     "[`app/src-tauri/src/db/migrations.rs`](../app/src-tauri/src/db/migrations.rs)"),
    ("docs/DEBUG_DB.md",
     "../IMPLEMENTATION/decisions.md",
     "./IMPLEMENTATION/decisions.md"),
    ("docs/DEBUG_DB.md",
     "../ARCHITECTURE.md",
     "./ARCHITECTURE.md"),
    ("docs/DEBUG_DB.md",
     "../HACKING-llm.md",
     "./HACKING-llm.md"),

    # ============ B1: HACKING 文档 _archive 路径 ============
    ("docs/HACKING-llm.md",
     "../_archive/2026-06-3b-1/FOLLOW-UP.md#fu-5--optiont-tauri-2-ipc-null-行为",
     "./_archive/2026-06-3b-1/FOLLOW-UP.md#fu-5--optiont-tauri-2-ipc-null-行为"),
    ("docs/HACKING-llm.md",
     "../_archive/2026-06-3b-1/FOLLOW-UP.md#fu-6--anthropic-tool_result-块只能出现在-user-role",
     "./_archive/2026-06-3b-1/FOLLOW-UP.md#fu-6--anthropic-tool_result-块只能出现在-user-role"),
    ("docs/HACKING-wsl.md",
     "../_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md",
     "./_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md"),
    ("docs/HACKING-wsl.md",
     "../_archive/2026-06-3b-1/FOLLOW-UP.md#fu-4--tauri-2-ipc-arg-默认-rename_all--camelcase",
     "./_archive/2026-06-3b-1/FOLLOW-UP.md#fu-4--tauri-2-ipc-arg-默认-rename_all--camelcase"),

    # ============ B1: STRUCTURE 标题归一(10 处,恢复 TOC 锚点) ============
    ("STRUCTURE.md", "##1.顶层结构", "## 1. 顶层结构"),
    ("STRUCTURE.md", "##2. 前端 `app/src/`树", "## 2. 前端 `app/src/` 树"),
    ("STRUCTURE.md", "##3. 后端 `app/src-tauri/src/`树", "## 3. 后端 `app/src-tauri/src/` 树"),
    ("STRUCTURE.md", "##4.关键模块依赖图", "## 4. 关键模块依赖图"),
    ("STRUCTURE.md", "##5. Tauri IPC表面", "## 5. Tauri IPC 表面"),
    ("STRUCTURE.md", "##6.数据库 schema", "## 6. 数据库 schema"),
    ("STRUCTURE.md", "##7. Tauri IPC事件表面", "## 7. Tauri IPC 事件表面"),
    ("STRUCTURE.md", "##8.关键设计模式", "## 8. 关键设计模式"),
    ("STRUCTURE.md", "##12.依赖与第三方集成", "## 12. 依赖与第三方集成"),
    ("STRUCTURE.md", "##13.文档地图 + 一页式 ASCII 全景", "## 13. 文档地图 + 一页式 ASCII 全景"),

    # ============ B2: IMPLEMENTATION.md#4-决策日志 → IMPLEMENTATION/decisions.md ============
    ("CLAUDE.md",
     "决策历史见 [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md#4-决策日志)。",
     "决策历史见 [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md)。"),
    ("CLAUDE.md",
     "决策动机见 [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md)，编排",
     "决策动机见 [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md)，编排"),
    ("README.md",
     "决策历史见 [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md#4-决策日志)。",
     "决策历史见 [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md)。"),
    ("README.md",
     "| 看\"为什么这么做\"的历史 ADR | [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md#4-决策日志) |",
     "| 看\"为什么这么做\"的历史 ADR | [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md) |"),
    ("docs/ARCHITECTURE.md",
     "决策见 [§4](#4-决策agent-daemon-化) + [IMPLEMENTATION.md §4](./IMPLEMENTATION.md)。",
     "决策见 [§4](#4-决策agent-daemon-化) + [IMPLEMENTATION/decisions.md](./IMPLEMENTATION/decisions.md)。"),
    ("docs/ARCHITECTURE.md",
     "[IMPLEMENTATION.md §4 \"2026-06-13 Re-grill ADR\"](./IMPLEMENTATION.md)",
     "[IMPLEMENTATION/decisions-2026-06.md \"2026-06-13 Re-grill ADR\"](./IMPLEMENTATION/decisions-2026-06.md)"),
    ("docs/ARCHITECTURE.md",
     "决策档案(为什么 axum / 为什么 sidecar / 为什么默认 httpTransport)见 [IMPLEMENTATION.md §4](./IMPLEMENTATION.md)。",
     "决策档案(为什么 axum / 为什么 sidecar / 为什么默认 httpTransport)见 [IMPLEMENTATION/decisions.md](./IMPLEMENTATION/decisions.md)。"),
    ("docs/CONTEXT.md",
     "走 `docs/IMPLEMENTATION.md §4` 决策日志",
     "走 `docs/IMPLEMENTATION/decisions.md` 决策日志"),
    ("docs/CONTEXT.md",
     "- 设计决策走 [`docs/IMPLEMENTATION.md §4 决策日志`](../IMPLEMENTATION/decisions.md)(本文件不重复)",
     "- 设计决策走 [`docs/IMPLEMENTATION/decisions.md 决策日志`](./IMPLEMENTATION/decisions.md)(本文件不重复)"),
    ("STRUCTURE.md",
     "详见 `docs/IMPLEMENTATION.md §4决策日志`。",
     "详见 `docs/IMPLEMENTATION/decisions.md`。"),
    ("STRUCTURE.md",
     "|实施后决策变更 | docs/IMPLEMENTATION.md §4决策日志 |",
     "|实施后决策变更 | docs/IMPLEMENTATION/decisions.md |"),
    ("docs/README.md",
     "| [IMPLEMENTATION.md](./IMPLEMENTATION.md) | 决策档案 | §1 自研 agent core 决策 + §4 决策日志(ADR 性质,只追加) | 想看\"为什么这么做\"的历史 ADR |",
     "| [IMPLEMENTATION.md](./IMPLEMENTATION.md) | 决策档案 | §1 自研 agent core 决策 + 决策日志(ADR 性质,只追加,按月分卷) | 想看\"为什么这么做\"的历史 ADR |"),
    ("docs/README.md",
     "- **写代码时反复查**:ARCHITECTURE.md §2 16 关卡 / TECH.md 选库 / IMPLEMENTATION.md §4 ADR",
     "- **写代码时反复查**:ARCHITECTURE.md §2 16 关卡 / TECH.md 选库 / IMPLEMENTATION/decisions.md ADR"),
    ("docs/IMPLEMENTATION.md",
     "详见 §4 [2026-07-20 — Agent daemon 化 + HTTP/SSE transport](#4-决策日志)。",
     "详见决策日志 [2026-07-20 — Agent daemon 化 + HTTP/SSE transport](./IMPLEMENTATION/decisions-2026-07.md)。"),

    # ============ 其余死锚修正 ============
    ("docs/DESIGN.md",
     "[IMPLEMENTATION.md §1](./IMPLEMENTATION.md#1-决策自己写-agent-runtime不用-sdk-包装)",
     "[IMPLEMENTATION.md §1](./IMPLEMENTATION.md#1-决策自己写-agent-core不用-sdk-包装)"),
    ("docs/DESIGN.md",
     "(详见 [BACKLOG §4.2](./BACKLOG.md#42-多模式mode))",
     "(详见 [权限层 spec](../.trellis/spec/backend/permission-layer.md))"),
    ("docs/DESIGN.md",
     "详见 [BACKLOG §7](./BACKLOG.md#7-云端状态同步) 和 [BACKLOG §9 跨设备](./BACKLOG.md#9-跨设备v2-候选)。",
     "详见 [BACKLOG §4 跨设备](./BACKLOG.md#4-跨设备)。"),
    ("docs/DESIGN.md",
     "这是 [⑨ Tool 权限](./ARCHITECTURE.md#9-工具权限检查) 实施的前提",
     "这是 [⑨ Tool 权限](./ARCHITECTURE.md#⑨-tool-权限检查) 实施的前提"),
    ("docs/HACKING-llm.md",
     "[TECH §2 rig-core](./TECH.md#2-决策rig-core-作为-llm-抽象层)",
     "[TECH §2 rig-core](./TECH.md#2-决策rig-core-弃用2026-06-09改自研-provider-trait)"),
    ("docs/HACKING-llm.md",
     "[IMPLEMENTATION §2.1 步骤 1](./IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp)",
     "[IMPLEMENTATION §1 自研决策](./IMPLEMENTATION.md#1-决策自己写-agent-core不用-sdk-包装)"),
    ("docs/ARCHITECTURE.md",
     "5a-5c 详解见 [BACKLOG.md §3 多层 Memory](./BACKLOG.md#3-多层-memory-与约束) 和 [BACKLOG.md §2 Skill](./BACKLOG.md#2-agent-skill-系统) 和 [BACKLOG.md §4.1 Role](./BACKLOG.md#41-多角色role)",
     "5a-5c 详解见 [memory spec](../.trellis/spec/backend/memory.md) 和 [BACKLOG.md §2 Skill](./BACKLOG.md#2-agent-skill-系统) 和 [ROADMAP §1.2 L3d](./ROADMAP.md#12-路线图外完成)"),
    ("docs/ARCHITECTURE.md",
     "| **ui_render**(新) | 到 ⑭ 走 UiCard(详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关)) |",
     "| **ui_render**(新) | 到 ⑭ 走 UiCard(详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成)) |"),
    ("docs/ARCHITECTURE.md",
     "- **详见 [BACKLOG.md §4.2 多模式](./BACKLOG.md#42-多模式mode)**",
     "- **详见 [permission-layer spec](../.trellis/spec/backend/permission-layer.md)**"),
    ("docs/ARCHITECTURE.md",
     "\"use_memory\"  => 读 / 写 runtime memory(详见 [BACKLOG §3](./BACKLOG.md#3-多层-memory-与约束))",
     "\"use_memory\"  => 读 / 写 runtime memory(详见 [memory spec](../.trellis/spec/backend/memory.md))"),
    ("docs/ARCHITECTURE.md",
     "\"use_ui\"      => 构造 UiCard 走 ⑭ 分支(详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关))",
     "\"use_ui\"      => 构造 UiCard 走 ⑭ 分支(详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成))"),
    ("docs/ARCHITECTURE.md",
     "- **Phase 1 范围**:4 种 primitive(button / selector / diff / code_block),详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关)",
     "- **Phase 1 范围**:4 种 primitive(button / selector / diff / code_block),详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成)"),
    ("docs/ARCHITECTURE.md",
     "触发云端同步(若开启,详见 [BACKLOG §7](./BACKLOG.md#7-云端状态同步))",
     "触发云端同步(若开启,详见 [BACKLOG §4](./BACKLOG.md#4-跨设备))"),
    ("docs/ARCHITECTURE.md",
     "- `FeishuChannel` — 走飞书 WebSocket(待 [BACKLOG.md §6](./BACKLOG.md#6-im-通道飞书) 实施)",
     "- `FeishuChannel` — 走飞书 WebSocket(B10 飞书 IM,待 [ROADMAP §2 第四档](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 实施)"),
    ("docs/TECH.md",
     "完整加载机制、token 预算、四层 Memory 边界见 [BACKLOG.md §3 多层 Memory](./BACKLOG.md#3-多层-memory-与约束) 和 [BACKLOG.md §2 Agent Skill 系统](./BACKLOG.md#2-agent-skill-系统)。",
     "完整加载机制、token 预算、四层 Memory 边界见 [memory spec](../.trellis/spec/backend/memory.md) 和 [BACKLOG.md §2 Agent Skill 系统](./BACKLOG.md#2-agent-skill-系统)。"),
    ("docs/REMOTE-ACCESS-ROADMAP.md",
     "[ARCHITECTURE §4/§5](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)",
     "[ARCHITECTURE §4/§5](./ARCHITECTURE.md#4-决策agent-daemon-化)"),
]


def main():
    n_hit = n_miss = 0
    for path, old, new in FIXES:
        full = f"{ROOT}/{path}"
        try:
            text = open(full, encoding="utf-8").read()
        except OSError as e:
            print(f"MISS-FILE {path}: {e}")
            n_miss += 1
            continue
        cnt = text.count(old)
        if cnt == 0:
            print(f"MISS {path}: {old[:70]!r}")
            n_miss += 1
        else:
            open(full, "w", encoding="utf-8").write(text.replace(old, new))
            print(f"OK {cnt}x {path}: {old[:45]!r} -> {new[:45]!r}")
            n_hit += cnt
    print(f"\ntotal: {n_hit} replacements, {n_miss} misses")


if __name__ == "__main__":
    main()
