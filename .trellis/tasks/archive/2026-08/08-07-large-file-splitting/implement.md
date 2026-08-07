# Implement — 大文件拆分(档位 1+2)与文档同步

> 执行顺序:Phase 0 基线 → Phase 1 Rust 拆分(5 项)→ Phase 2 前端拆分(2 项)→ Phase 3 文档(更新+拆分+链接)→ Phase 4 终验。每个拆分项独立 commit、独立回滚点。

## Phase 0 — 基线

- [ ] 0.1 全量基线确认:`cargo test --lib`(需 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`,不要 `--test-threads=1`)+ `pnpm test` 全绿;记录各耗时
- [ ] 0.2 **记录测试计数基线 N**:`cargo test --lib` 输出末尾 `test result: ok. N passed` 的 N 值 + `pnpm test` 计数,写入任务 notes;Phase 4.1 对照 N 不变(比"全绿"更硬)
- [ ] 0.3 确认前端 typecheck 命令:无独立 script,`build` 内含 `vue-tsc --noEmit` → 用 `cd app && pnpm exec vue-tsc --noEmit`(写进 §速查)
- [ ] 0.4 `git status` 干净;确认分支策略(建议 `refactor/file-splitting`,与用户确认)
- [ ] 0.5 记录各目标文件当前行数(wc -l)作为拆分前后对照

## Phase 1 — Rust 拆分(每项:拆分 → 模块测试 → 全量 → commit)

每项验证命令:`cargo test --lib`(全量,~26s)+ `cargo clippy --lib` + `cargo fmt --check`(零警告)

1. **wire.rs → wire/ 目录**(design §1.1)
   - [ ] 建 `wire/{mod,types,out,in}.rs`,类型/双向转换搬移,re-export 保持调用点
   - [ ] 内嵌测试 → `llm/provider/tests_wire.rs`(provider/mod.rs 声明)
   - [ ] 验证 + commit `refactor(wire): 双向转换拆 wire/{types,out,in} + 测试迁出`
2. **group_chat_loop.rs → + group_chat_prompts.rs**(design §1.2)
   - [ ] **先 `wc -l agent/tests_group_chat.rs` 确认现状(919 行,08-04 集成测试,不并入)**
   - [ ] 纯函数簇搬移;role_history/prompt 测试随迁新文件内嵌 mod tests
   - [ ] 其余循环逻辑内嵌测试 → 新建 `agent/tests_group_chat_loop.rs`(agent/mod.rs 声明,评审 P1-1)
   - [ ] 验证 + commit `refactor(group-chat): prompt/历史纯函数簇拆 group_chat_prompts.rs`
3. **loader.rs → + frontmatter.rs + cache.rs**(design §1.3)
   - [ ] 搬移;测试随迁;其余 → `subagent/tests_loader.rs`
   - [ ] 验证 + commit `refactor(subagent): loader 拆 frontmatter/cache + 测试迁出`
4. **openai.rs → + streaming.rs**(design §1.4)
   - [ ] 搬移;测试随迁;其余 → `llm/provider/tests_openai.rs`
   - [ ] 验证 + commit `refactor(llm): openai 流式 tool-call 装配簇拆 streaming.rs`
5. **dispatch.rs → + resolve.rs + worktree.rs**(design §1.5)
   - [ ] 搬移;测试随迁;其余 → `agent/subagent/tests_dispatch.rs`(subagent/mod.rs 声明,fn 升 pub(crate))
   - [ ] 验证 + commit `refactor(subagent): dispatch 解析簇拆 resolve.rs + worktree.rs`

## Phase 2 — 前端拆分

6. **MessageItem.vue → messageCards/*.ts + messageTimeline.ts**(design §1.8)
   - [ ] 3 个卡片解析簇 + 时间轴纯 TS 提取;.vue 内 import
   - [ ] `pnpm test` + typecheck 通过;commit `refactor(chat-ui): MessageItem 卡片解析簇拆 messageCards/*`
7. **streamController.ts → + streamEvents.ts**(design §1.7)
   - [ ] 事件处理块搬移;store re-export 保 return 块导出;两个测试文件适配
   - [ ] `pnpm test`(streamController 过滤 + 全量)通过;commit `refactor(chat-store): 事件处理块拆 streamEvents.ts`

## Phase 3 — 文档(更新 → 拆分 → 链接)

8. **R2 过时文档更新**:对 Phase 1/2 每个拆分后 sweep 的引用逐条更新
   - [ ] `agent-loop-architecture.md` Group-chat Pattern 的 `group_chat_loop.rs` 路径/行号
   - [ ] 其他被搬符号/路径引用(ARCHITECTURE/IMPLEMENTATION/ROADMAP/spec 互引)
9. **文档拆分**(design §2,hub+parts,6 个文件)
   - [ ] **先处理 `tool-contract.md` 的 `request_mode_change` Scenario 去重**(L2599/L3063 两份,评审 P2-3),再去重后按 Scenario 分组拆分
   - [ ] `tool-contract.md`(按 Scenario 分组)
   - [ ] `memory.md` / `agent-loop-architecture.md` / `multi-provider-contract.md`
   - [ ] `docs/IMPLEMENTATION.md`(注意 ROADMAP §4 锚点 + §4 内 `chat_loop.rs:657` 行号引用随迁,评审 P3-2)/ `docs/WORKFLOW-INTEGRATION.md`
10. **R4 链接修复**:拆分后全仓 sweep 失效锚点/路径;**检查清单显式包含**(评审 P3-2):`RULE-A-006` / `chat_loop.rs` 行号引用(`tool-contract.md:1073`、`docs/IMPLEMENTATION.md:486`、`debt-status-evolution-guide.md` 等);`grep -rn "#\|\.md"` 抽查 + 逐个验证
11. commit docs:`docs: 拆分超长文档(hub+parts)+ 修复拆分后失效链接`

## Phase 4 — 终验

- [ ] 4.1 全量:`cargo test --lib` + `pnpm test` + `cargo clippy` + `cargo fmt --check` + `pnpm exec vue-tsc --noEmit` 全绿;**测试计数与 Phase 0.2 基线 N 一致**
- [ ] 4.2 行数对照:`wc -l` 确认每个目标文件拆分后 <1200 行(记录到 commit message 或任务 notes)
- [ ] 4.3 纯搬迁核对:逐 commit `git show <commit> --stat` 核对"新增文件行数 + 删除 ≈ 原文件行数",配合 `git diff -w --ignore-all-space` 对比函数体无语义改动
- [ ] 4.4 `trellis-check`(spec 合规 + 跨层一致性)
- [ ] 4.5 更新 PRD 验收清单打勾

## 回滚点

- 每个 commit 独立可 `git revert`;拆分顺序从最干净到最复杂,前项失败不影响后项
- 风险最高的两个项(streamController/ dispatch 测试可见性)放最后,失败可单独回滚

## 验证命令速查

```bash
# 后端(AGENTS.md 坑 1)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 前端
cd app && pnpm test
cd app && pnpm exec vue-tsc --noEmit   # typecheck(无独立 script,build 内含)
# 文档引用 sweep
grep -rn "<旧路径>" docs/ .trellis/spec/ AGENTS.md .trellis/workflow.md .trellis/tasks/ --include="*.md" | grep -v archive/
```
