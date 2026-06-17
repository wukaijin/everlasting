# B4 Skill 系统(use_skill 虚拟 tool + 三层渐进披露)

## Goal

把"做某件事的方法"打包成可复用单元,既能被 LLM 按需调(`use_skill` 虚拟 tool)。对齐 Claude Code / Hermes 的"虚拟 tool + 渐进式披露"业界模式,复用已落地的 B3 `/command` ResourceLoader 加载层,作为**指令层扩展**进第三档(ROADMAP §2 🟠 B4)。

## What I already know

- **前置调研**:[`docs/research/skill-system-survey.md`](../../../docs/research/skill-system-survey.md) — Claude Code / Hermes / opencode / agentskills.io 一手抓取
- **业界共识**:虚拟 tool 模式(非 system prompt 全量注入);三层渐进披露 L0(清单常驻)/L1(正文按需)/L2(reference 文件 read_file 拉)
- **BACKLOG §2 两处过时已修正**:① 选型 `serde_yml`→废弃(B3 手写 parser);② "注入 system prompt"→"注入消息流"
- **DEBT**:0 P0 open,1 P1 open(`RULE-D-001` API key 明文,正交不阻塞)

## Decisions (brainstorm 已收敛)

1. **✅ MVP = 纯 LLM 自动触发**:只做 `use_skill` 虚拟 tool,最小闭环验证 L0→L1→L2。不做用户手动 `/skill`、不做 `allowed-tools`。frontmatter 最小集 `name`+`description`。
   - ⇒ parser 复用 B3 手写(零新依赖);tool list 静态注册(不做 `disable-model-invocation`)
2. **✅ 加载层 = 独立 SkillCache**:复制 B3 的 `CommandCache`+`parse_frontmatter`+`scan_dir` 模式,新建 `app/src-tauri/src/skill/`。B3 零改动零回归(~200 行结构重复,稳定后再抽 `ResourceLoader<Kind>` 泛型)。
3. **✅ L0 清单注入 = 独立第二条 synthetic message**:skill 清单作为独立 synthetic user message,自带 `cache_control: Ephemeral` 断点,追加在 memory synthetic message(`chat_loop.rs:235-248`)之后。与 memory 解耦——skill 增删不破坏 memory 缓存。
4. **✅ L1 正文注入 = tool_result 回填**:`execute_tool("use_skill")` 返回正文,走现有 ⑫ tool_result 回填路径。零新代码路径,自动进 ⑩ 审计,C3 compaction 配对保护直接复用。

## Requirements

- `use_skill` 虚拟 tool 注册进 `AppState.tools`,模型可在相关任务自发调用
- skill 清单(`{name, description}`)在 session 启动时作为独立 synthetic message 注入(L0 常驻)
- `use_skill(name)` 执行返回 SKILL.md 正文,经 tool_result 注入后续上下文(L1)
- 复用 B3 ResourceLoader 加载层:`SkillCache` mtime fence + `parse_frontmatter`(手写) + precedence
- 两层路径:user(`~/.config/everlasting/skills/<name>/SKILL.md`)+ project(`<project>/.everlasting/skills/<name>/SKILL.md`),project 覆盖 user
- `name` 约束对齐 agentskills.io:小写+连字符(无 frontmatter 时从目录名推导,对齐 B3 stem fallback)

## Acceptance Criteria

- [ ] 定义一个 skill 后,LLM 在匹配任务中能自发调 `use_skill`(集成测试:MockProvider emit tool_use → 正文进 tool_result)
- [ ] `use_skill` 返回的 skill 正文进入后续 agent loop 轮次的上下文
- [ ] skill 文件 mtime 变更后下次请求自动重载(read-through fence,对齐 B3 `read_through`)
- [ ] user/project 同名 skill 时 project 胜出(precedence 测试)
- [ ] `use_skill("不存在")` 返回 `is_error=true`(LLM 自纠错路径)
- [ ] 无 skill 文件时不注入清单 message(对称 `build_banner` 空时 skip)
- [ ] Plan 模式放行 `use_skill`(黑名单制 `filter_tools_for_mode` 自动覆盖,无需额外代码)
- [ ] `SkillCache` 单测覆盖:parse / scan / mtime fence / precedence(对齐 `resource_loader` tests)

## Definition of Done

- 单元测试(`skill/loader.rs`,对齐 `resource_loader.rs` 的 14 个测试形态)
- 集成测试(`agent_loop_use_skill_*`,走通 tool_use → tool_result 正文)
- `PKG_CONFIG_PATH=... cargo test --lib` 绿
- ROADMAP 第三档 B4 划掉 + IMPLEMENTATION §4 ADR + DEBT 无新增

## Technical Approach

### 数据流

```
session 启动 (chat_loop.rs)
  └─ SkillCache.list_all() [mtime fence]  → [{name, description}, ...]
  └─ skill::loader::build_skill_listing_block()
      → 独立 synthetic user message (cache_control: Ephemeral)
      → 追加在 memory synthetic message 之后

每轮 agent loop
  └─ LLM 看 tool list 有 use_skill + 上方清单
      → 命中 → emit tool_use("use_skill", {skill_name})
  └─ execute_tool_inner "use_skill" 分支
      → SkillCache 取 SKILL.md 正文 (L1)
      → 返回 (正文, is_error=false)
  └─ ⑫ tool_result 回填 → 正文进 messages 常驻
  └─ LLM 拿正文按指令执行 (L2 reference 文件用 read_file 拉)
```

### 精确接入点(均已定位行号)

| 接入点 | 文件:行 | 动作 |
|---|---|---|
| L0 清单注入 | `agent/chat_loop.rs:235-248` 后 | 追加 skill 清单 synthetic message |
| 虚拟 tool 注册 | `tools/mod.rs:37` `builtin_tools()` | 追加 `skill::definition()` |
| `use_skill` 执行 | `tools/mod.rs:140` `execute_tool_inner` match | 加 `"use_skill"` 分支 |
| SkillCache 持有 | `state.rs:76` + `state.rs:177` `load` | 加 `skill_cache` 字段 + 初始化 |

### Edge case 处理(全部复用现有机制)

| 场景 | 处理 | 依据 |
|---|---|---|
| skill 文件损坏/超大 | bad-file skip + `warn!` | B3 `scan_dir`(`resource_loader.rs:243`) |
| 同名冲突 | project > user precedence | B3 `list_all` |
| 调不存在的 skill | 返回 `is_error=true` | LLM 自纠错(⑫ 错误回填) |
| 无 skill 文件 | 不注入清单 message | 对称 `build_banner` 空时 skip |
| Plan 模式 | 自动放行 | `filter_tools_for_mode` 黑名单制(:1355) |
| skill 正文大小 | 复用 64KB cap(`MAX_SKILL_FILE_SIZE`) | B3 `MAX_COMMAND_FILE_SIZE` |
| L2 reference 文件 | 模型 `read_file` 拉;**user-layer reference 受 read_file boundary 限制**(project root 外),MVP 接受(SKILL.md 应自包含) | `read_file` boundary check |

## Implementation Plan (小 PR)

- **PR1 — skill 加载层**(`app/src-tauri/src/skill/`):`SkillResource`/`SkillCache`(mtime fence)/`scan_dir`/`parse_frontmatter`(复用 B3 手写)/`list_all`(precedence)/`build_skill_listing_block` + `SkillCache` 进 `AppState`。单测对齐 `resource_loader.rs`。**不接 agent loop**。
- **PR2 — 接入 agent loop**:`use_skill` tool(`tools/skill.rs` definition + execute)+ `execute_tool_inner` 分支 + `builtin_tools()` 注册 + `chat_loop.rs` L0 清单注入 + 集成测试 + 文档(ROADMAP/IMPLEMENTATION ADR)。

## Out of Scope (explicit)

- `allowed-tools` / `disallowed-tools`、`disable-model-invocation` / `user-invocable` 开关、用户手动 `/skill` 入口
- L0 清单预算制(1% 窗口)—— skill 数量少时先不做
- L2 user-layer reference 的 boundary 豁免(MVP 接受限制)
- 条件激活(Hermes `requires_toolsets`)、自我创建 skill(Hermes `skill_manage`)、cron/blueprint
- skill 与 command 泛型合并(稳定后再 refactor)
- 与 B6 Subagent 联动(`context:fork` 驱动 subagent)

## Decision (ADR-lite)

**Context**: 第三档 B4 需要把"做事方法"打包成可复用单元,业界(Claude Code/Hermes)已收敛到"虚拟 tool + 渐进披露"。本仓库 B3 `/command` 已落地 ResourceLoader,MEMORY 系统已有 synthetic message 注入机制。

**Decision**: 采用虚拟 tool `use_skill` + 三层披露(L0 独立 synthetic message / L1 tool_result 回填 / L2 read_file),加载层独立 `SkillCache` 复用 B3 模式(不动 B3),frontmatter 最小集 `name`+`description`。正文注入消息流而非 system prompt(修正 BACKLOG §2,保 cache_control 结构)。

**Consequences**: 上下文成本可控(清单常驻,正文按需);权限/审计/compaction 全复用现有 ⑨/⑩/⑫/C3 通道;代价是加载层 ~200 行与 B3 结构重复(YAGNI 当前,稳定后抽泛型)。

## Research References

- [`docs/research/skill-system-survey.md`](../../../docs/research/skill-system-survey.md) — 业界虚拟 tool 模式 + 三层披露 + 注入位置行号 + 与 command 复用边界

## Technical Notes

- BACKLOG §2/§3 早期构思:`docs/BACKLOG.md`(已按本 PRD 修正两处过时)
- ARCHITECTURE ⑩ 表 `use_skill` 占位:`docs/ARCHITECTURE.md:480`
- B3 ResourceLoader(复用基准):`app/src-tauri/src/resource_loader.rs`
- memory loader 注入点(复用基准):`app/src-tauri/src/memory/loader.rs:325`
- DEBT:`RULE-D-001`(P1 open,API key 明文,正交不阻塞)
