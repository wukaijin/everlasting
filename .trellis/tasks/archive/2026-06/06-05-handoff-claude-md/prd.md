# 更新过时项目文档（HANDOFF/CLAUDE.md）

## Goal

根据 git log 实际状态，更新仓库里跟"当前进度/已完成步骤"挂钩的过时文档。让"读完文档"就知道项目真实在哪一步，避免下次 session 按陈旧描述假设。

## What I already know

### 实际 git log 显示的代码进度

| commit | 内容 | 是否在 IMPLEMENTATION 路线图里 |
|---|---|---|
| `08dc818` | MVP 步骤 1 — Tauri 2 + Vue 3 + LLM 直连骨架 | §2.1 |
| `fefc41f` | 步骤 2 — Tool Calling + Agent Loop | §2.2 |
| `0ce44b5` | 步骤 3a — SQLite + Session 持久化 | §2.3 |
| `a89a6fd` | fix: session 切换 rehydrate + 持久化 user 消息（3a 补丁） | — |
| `281e51b` | docs(spec): LLM API contract + extended thinking state | — |
| `05671f5` | feat(thinking): 步骤 6 — Anthropic extended thinking | ❌ **路线图外**（commit message 写"步骤 6"但 §2.7 步骤 6 是 MCP） |
| `402afa5` | initial trellis + .claude + AGENTS.md setup | — |

**步骤 3b（多项目 + UI 三栏 + Rig 迁移）跳过没做**，直接做了 extended thinking。

### 扫描结果：过时点清单

#### 明确过时（事实型，必改）

- [ ] **A1. `CLAUDE.md:9`** "当前状态：MVP 步骤 1（骨架 + LLM 直连）已完成。" — 严重过时
- [ ] **A2. `docs/HANDOFF.md` 顶部** "MVP 步骤 3a 已完成，准备进入步骤 3b" + "session 4 未 commit" — 跟 git 不符
- [ ] **A3. `docs/HANDOFF.md` §2 已完成清单** 只列到步骤 3a，没列 extended thinking
- [ ] **A4. `docs/HANDOFF.md` §2 当前任务** "下一步 → 步骤 3b" + "记得先 commit session 4 改动" — 已不准
- [ ] **A5. `docs/HANDOFF.md` §3 阅读顺序表** "了解 MVP 1 范围"等措辞针对刚起步阶段，已不合适
- [ ] **A6. `docs/HANDOFF.md` §4 整节** "MVP 步骤 1 是什么 + 起点 + 验收"——大段历史引导，已完成；保留为历史 vs 收编入"已完成"段，待定
- [ ] **A7. `docs/HANDOFF.md` §8 最近 commit hash + 当前日期** `1bcc9e8` → 当前 `da325a4`，日期 2026-06-04 → 2026-06-05
- [ ] **A8. `docs/IMPLEMENTATION.md` §2.2 / §2.3** 步骤 2、3a 标题没加 ✅ 已完成
- [ ] **A9. `docs/IMPLEMENTATION.md` §3 "最后更新"** "2026-06-04(步骤 1 完成…)" 过时
- [ ] **A10. `docs/IMPLEMENTATION.md` §3 "下一步"** → 步骤 2 早完成
- [ ] **A11. `docs/IMPLEMENTATION.md` §3 路线图全貌表** 步骤 2 / 3a 没标 ✅、"← **当前**"指针停在步骤 2
- [ ] **A12. `docs/IMPLEMENTATION.md`** 未体现"路线图外完成：extended thinking"——应该补一条
- [ ] **A13. `docs/README.md:22`** HACKING-wsl "**5 个**已知坑" 实际是 10 个
- [ ] **A14. `docs/IMPLEMENTATION.md` 决策日志** 应记一条"步骤 6 编号语义冲突"或"步骤 3b 暂缓 + extended thinking 插入路线图外"

#### 可疑点（请你勾选改不改）

- [ ] **B1. `docs/DESIGN.md` §3.1 MVP tools 列表** `read_file / write_file / edit_file / shell / grep / glob`——实际只实现 read_file / write_file / shell。但 §3.1 列的是"MVP 目标"不是"已完成"，可能是有意保留（计划要做）。**改 vs 不改？**
- [ ] **B2. `docs/ARCHITECTURE.md` §2.4 实施映射表** 编号语义和 IMPLEMENTATION 不一致：ARCHITECTURE 里"步骤 6 = daemon 化"，IMPLEMENTATION §2.7 "步骤 6 = MCP + 多 Provider"，commit "步骤 6 = thinking"，三处冲突
- [ ] **B3. `docs/ARCHITECTURE.md` §5 "当前实现"** 列了 3 个 channel（TauriGui / Feishu / Cli），实际只有 TauriGui。Feishu 标了"待…"，Cli 没标"待"
- [ ] **B4. `docs/DESIGN.md` §5.1 风险表** Tauri 2 WSLg bug / Linux sandbox 风险——spike-001 通过后 WSLg 已验证可用，是否要标"已验证"或"已缓解"？
- [ ] **B5. `docs/HANDOFF.md` 整体结构** §4 大段"步骤 1 起点 + 验收"已是历史。要不要重写 HANDOFF 让它变成"会自动跟上"的形态——比如把"当前进度"段改成"参考 IMPLEMENTATION §3 + `git log --oneline -10`"自助式？

#### 不动（明确不在范围）

- `docs/HACKING-wsl.md` / `docs/HACKING-llm.md` — 不含进度型陈述
- `docs/TECH.md` — grep 0 个 match，不含进度型陈述
- `docs/REVIEW-glm-5.1.md` / `docs/REVIEW-deepseek-v4-pro.md` — 外部评审快照，不应改
- `docs/spikes/*.md` — 历史 spike 验证记录，固化
- `docs/BACKLOG.md` — 候选功能描述，纯方案型；用户颗粒度选了"严格"，BACKLOG 现状对齐不在范围

## Assumptions (temporary)

- HANDOFF 重写要保留"5 分钟上手 + 工具链状态 + 关键决策 + 撞过的坑"等永久有用段落
- IMPLEMENTATION 不重组 7 步骨架，只在原结构上 ✅ 进度 + 补"路线图外"段 + 决策日志加一条
- "步骤 6"语义冲突在 IMPLEMENTATION.md 决策日志里记一条说明，不强行改 commit message 或 §2.7 标题

## Open Questions

（已收敛 — 用户在 Step 8 final confirmation 全勾 B1-B5）

## Requirements

- A1-A14 全部改（事实型过时）
- B1 DESIGN §3.1 MVP tools 列表：edit_file / grep / glob 标"未实现"或调整说法
- B2 ARCHITECTURE §2.4 实施映射表：加 thinking 的 footnote
- B3 ARCHITECTURE §5 channel 列表：Cli 加"（待后期）"标
- B4 DESIGN §5.1 风险表：spike-001/002 验证过的风险加"已验证"标
- B5 HANDOFF §4 整段重写成"参考 IMPLEMENTATION + git log"自助式
- 不动的文档列表见上

## Acceptance Criteria (evolving)

- [ ] `grep -nE "(步骤 ?1.*已完成|当前.*步骤 ?[0-9]|准备.*步骤 ?3b|session 4.*未 commit)" CLAUDE.md docs/` 应**无残留**过时句
- [ ] HANDOFF "当前进度" / "下一步" / "最近 commit hash" 与 `git log --oneline -1` 一致
- [ ] IMPLEMENTATION §2.2 §2.3 显示 ✅ 已完成；§3 路线图全貌表完成项打勾、当前指针正确
- [ ] IMPLEMENTATION 有一段（§2.x 或 §3 或 §4 决策日志）显式说明"extended thinking 在路线图外完成 + 步骤 3b 暂缓"
- [ ] README.md HACKING-wsl 描述里"5 个已知坑"改成实际数（10）
- [ ] 不改 README / DESIGN / ARCHITECTURE / TECH / BACKLOG / HACKING-* / spikes / REVIEW 中 B1-B5 之外的内容

## Definition of Done

- 改动只动 docs/ 和 CLAUDE.md，不动代码
- 改完 `git diff` 自审一遍
- 改完再 grep 一次过时短语，确认全清
- 改完不需要 commit（留给用户自己 commit）

## Out of Scope (explicit)

- 不改代码、不改 spec、不改 trellis 配置
- 不重新编号 "步骤 6"（在决策日志记一条说明即可）
- 不补新 spike 文档
- 不大规模重构 HANDOFF 结构（除非 B5 勾选）

## Technical Notes

- 文档目录扁平，所有 doc 在 `docs/` 下，索引在 `docs/README.md`
- `CLAUDE.md` 在仓库根，是 Claude Code 项目级 instructions
- HACKING-llm / HACKING-wsl 不带进度语句，无需动
- 改完不需要跑构建/测试，纯 markdown
