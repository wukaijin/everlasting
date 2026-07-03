# Implement — A2+ P1+P2

> **设计**:[`design.md`](./design.md) · **prd**:[`prd.md`](./prd.md) · **source**:[`docs/A2-SHELL-CLASSIFICATION.md`](../../../docs/A2-SHELL-CLASSIFICATION.md) §4+§6
>
> 执行清单。P1+P2 同 PR,内部分两个 commit(P1 先、P2 后)便于二分回滚。

## 前置

- 工作目录:`app/src-tauri/`
- 测试命令(WSL,见 CLAUDE.md HACKING-wsl 坑 1):
  ```
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  ```
- 快速编译检查:`PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

## Step 1 — P1 `detect_write_redirect`(shell_trust.rs)

- [ ] 实现 `detect_write_redirect(seg: &str) -> bool`(design §3.5)
- [ ] 单元测试(内联 `#[cfg(test)]`):`>`/`>>`/`&>`/`2>err` 升;`2>&1`/`>&2`/`<`/`<<`/`<<<` 不升;引号内 `>`(`echo ">"`)不升
- [ ] validate:`cargo test detect_write_redirect`
- **回滚点**:函数独立,未接入 classify,可单独保留/删除

## Step 2 — P1 grant 短路前置(check.rs)

- [ ] shell_trust.rs 加 `pub(crate) fn has_structural_metachar(cmd: &str) -> bool`(design §3.1)
- [ ] check.rs L298 `(a) prefix-grant` 包进 `if !has_structural_metachar(cmd) { ... }`
- [ ] check.rs L307 worker run-grant 同款包裹
- [ ] `tests_check.rs` 新增:`ls`+grant → Allow;`ls; rm`+grant → 不短路(回落 classify → Ask);worker 路径用 `RunGrantCache` 同款
- [ ] validate:`cargo test --lib`(现有 grant 测试应仍绿)
- **回滚点**:revert check.rs 两处包裹

## Step 3 — P2 拆分器 `split_top_level`(shell_trust.rs)

- [ ] 实现 `split_top_level(cmd: &str) -> Vec<&str>`(design §3.3 状态机,4 态)
- [ ] 单元测试:引号内元字符不拆(`echo "a;b"`)/ `&&`/`||`/`;`/`|` 拆 / 空段跳过(`a ;; b`)/ 转义(`echo a\;b`)/ 双引号内 `\` 仅对 `$` `` ` `` `"` `\` 转义
- [ ] validate:`cargo test split_top_level`
- **回滚点**:函数独立,未接入

## Step 4 — P2 classify_single + 入口重写(shell_trust.rs)

- [ ] 提取 `classify_single(seg: &str) -> ShellTrust`:现有 `classify_prefix` L369-386(git 子命令 + 白名单 + 默认 Ask)+ 叠加 `detect_write_redirect`(design §3.4)
- [ ] 实现 `has_command_substitution(cmd: &str) -> bool`(design §3.2)
- [ ] `ShellTrust` 加 `severity(self) -> u8` + 自由函数 `max_of(a, b)`(design §3.6)
- [ ] 重写 `classify_prefix` 入口(design §3.7):empty → has_cmd_sub → split → map(classify_single) → reduce(max_of)
- [ ] **删除** L356-367 一刀切结构降级
- [ ] validate:`cargo test --lib`

## Step 5 — 现有结构降级测试断言重判

> P2 精化的预期变化,逐条对照 design §5 表。**先跑测试看实际失败,再据表改断言**。

- [ ] `classify_pipe_downgrades_to_ask`:`ls | grep foo` / `git status | head -5` / `cat x | head` → **ReadOnly**;`git log | bash` 仍 Ask。考虑拆成 `classify_pipe_*` 多测
- [ ] `classify_logical_and_downgrades_to_ask`:`git diff && cargo build` → **SideEffect**;`ls && echo done` → **ReadOnly**;`ENV=noop && cargo check` → 先确认 `first_token("ENV=noop")` 实际取值再定(可能 Ask)
- [ ] `classify_logical_or_downgrades_to_ask`:`cargo fmt || true` → **SideEffect**;`git diff || echo nope` → **ReadOnly**
- [ ] `classify_sequence_downgrades_to_ask`:`cd foo; ls` 仍 Ask(cd 非 whitelist);`echo a; echo b` → **ReadOnly**
- [ ] 重命名测试函数(原 `_downgrades_to_ask` 名字不再普遍准确)→ 如 `classify_compound_*`,或在新矩阵测试里覆盖、保留旧名改断言
- [ ] validate:`cargo test --lib` 全绿

## Step 6 — 新增复合矩阵测试(prd AC 11 项)

- [ ] 纯读单条不回归:`ls`/`git diff`/`cat x` → ReadOnly
- [ ] 纯读复合 → ReadOnly:`ls | grep foo`/`git diff | head`/`cat x | wc -l`
- [ ] 读+Ask 段 → Ask:`ls; rm x`/`ls || rm x`
- [ ] 读+SideEffect 段 → SideEffect:`git diff && cargo build`(cargo SideEffect)
- [ ] 写重定向 → SideEffect:`git diff > patch.txt`/`echo hi >> log`/`cmd &> f`
- [ ] `2>&1`/`<` 不误升:`cmd 2>&1 | head` 整体仍 ReadOnly(`2>&1` 不升,管道两 ReadOnly);`cat < /etc/hostname` ReadOnly
- [ ] 命令替换 → Ask:`ls $(rm x)`/`` ls `rm x` ``
- [ ] 引号内元字符不误拆:`echo "a;b"`/`grep "a|b" f` → 按 echo/grep 单条判
- [ ] 空命令/单条:`""`→Ask;`ls`→ReadOnly
- [ ] grant 短路收紧:`ls`+grant→Allow;`ls; rm x`+grant→不短路→Ask(在 tests_check.rs)
- [ ] worker 路径同款(在 tests_check.rs 或 tests_run_grant.rs,用 `RunGrantCache`)
- [ ] validate:`cargo test --lib` 全绿

## Step 7 — 回归 + 文档 + 提交

- [ ] `PKG_CONFIG_PATH="..." cargo test --lib` 全绿
- [ ] `cargo check` 无新 warning
- [ ] spec 更新:`.trellis/spec/backend/tool-contract.md` "Scenario: Path-based Permission" 段补复合命令判定契约(拆分取 max + grant 短路前置 + `>` 升 SideEffect)
- [ ] ROADMAP:`docs/ROADMAP.md` A2+ 条目从第三档移到 §1.2 已实施 + commit hash
- [ ] ADR:`docs/IMPLEMENTATION.md §4` 决策日志加选型(自研拆分器 vs tree-sitter-bash vs 沙盒优先;讲清为什么 P1+P2 同 PR、P3 远期)
- [ ] 提交(commit message 引用 task slug `a2-shell-p1p2-classify`,P1/P2 分两个 commit)

## Review gate(step 1.4 `task.py start` 前)

- [ ] prd / design / implement 三件套 review 过关
- [ ] prd Constraints 7 条不变量无违反
- [ ] prd AC 11 项回归矩阵全部可测
- [ ] 现有结构降级测试断言变化已在 design §5 表对齐(无遗漏)

## 完成后(parent 集成)

- [ ] parent `07-04-a2-shell-classification` cross-child 验收清单逐项打勾
- [ ] archive 本子任务 → parent 做最终集成 review → archive parent
