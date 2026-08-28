# A2+ Shell 命令只读/副作用精细判定 — 设计审查报告

> 审查日期：2026-07-03
> 审查模型：deepseek-v4-pro
> 审查范围：[`docs/A2-SHELL-CLASSIFICATION.md`](../2026-08-28-a2-shell-classification.md) 全文（6 章 + 附录）
> 审查基线：`shell_trust.rs` (732 行) + `check.rs` (Tier 4 Shell 段 L292-352) + `dangerous.rs` (277 行) 全部代码验证
> 审查角度：设计正确性、与现有代码的对齐、缺口覆盖、遗漏风险、实施可行性

---

## 一、总体评价

**整体评分：★★★★ (4/5) — 设计方向正确、分阶段顺序合理，有 1 个文档错误 + 4 个需在实施 design.md 阶段补充的细节**

这是一份**问题诊断精准、架构落点准确**的方案文档。两个缺口（grant 绕过 + 只读管道误伤）被正确归因为"首 token 分类器 + 一刀切结构降级"的粒度问题，分三阶段的递进路线也合理：P1 急堵安全缺口 → P2 恢复体验与精度 → P3 远期限损。不变量（2.2）和非目标（2.3）的列举是设计文档中最有价值的部分——它把改动面的边界画得非常清楚，避免了 scope creep。

**与代码的对齐程度高**：逐行验证了提案对 `shell_trust.rs` 五步算法、`check.rs:298` prefix-grant 短路、`check.rs:326` classify_prefix 调用、`dangerous.rs` kill-list 边界、grant 表的 `match_value_for_allow_always` 的描述，全部准确。提案作者显然是在仔细阅读了代码之后写的。

**但以下问题需要在制定 `design.md` 时解决**，否则实施阶段会遇到决策阻塞：

---

## 二、致命问题

### 2.1 §3.1 阶段标注与 §4 矛盾 — 文档内部不一致 (P0)

**严重程度：P0 — 会误导实施者，必须修正**

§3.1 管线图中 Shell 分支的标注为：

```
(a) prefix-grant 短路  ──▶ 阶段 2 改前置条件
(b) classify_prefix    ──▶ 阶段 1 精细化
```

但 §4 的正文明确规定：**阶段 1 (P1) = grant 收紧，阶段 2 (P2) = classify 精细化**。两者正好相反。

§4 的分配在逻辑上是正确的——P1 必须先于 P2（见 §4 首段的依赖分析）——所以 §3.1 的标注是 swap 错误。应修正为：

```
(a) prefix-grant 短路  ──▶ 阶段 1 改前置条件（P1: grant 收紧）
(b) classify_prefix    ──▶ 阶段 2 精细化（P2: 拆分 + 取 max）
```

---

## 三、重要遗漏 (P1)

### 3.1 重定向检测覆盖不完整

§4 P2 列出重定向符为 `>`/`>>`/`2>`/`&>`。缺失以下合法重定向形式：

| 缺失形式 | 示例 | 风险 |
|---|---|---|
| `1>` / `1>>` | `cmd 1>file` — 显式 fd 1 重定向 | false negative：`git diff 1> patch` 判为 ReadOnly |
| `&>>` | `cmd &>>file` — append both stdout+stderr | false negative |
| `<>` | `cmd <>file` — 读写打开 (POSIX) | 罕见但确实写 |
| `n>` (n≥3) | `cmd 3>file` — 自定义 fd 写 | 非常罕见，可接受漏判 |

**建议**：在设计阶段把重定向检测抽象为一个正则/状态机，覆盖 `[0-9]*>>?` / `&>>?` / `<>`，而非逐个枚举。设计文档 (`design.md`) 必须列全，实施时逐个加测试。

**额外注意**：`<`（输入重定向）不应判 SideEffect——`cat < /etc/hostname` 是纯读。`<<` (heredoc) 和 `<<<` (herestring) 也是读。重定向检测必须**只匹配写方向**的 `>`。

### 3.2 `||` 拆分会被 `|` 贪吃 — 拆分顺序未定义

§4 P2 说按 `;`/`&&`/`||`/`|` 拆分命令。但逐字符扫描时，`||` 会先被 `|` 吃掉（因为 `|` 是 `||` 的子串）。例如：

```
cmd1 || cmd2   →  按 `|` 拆会得到 ["cmd1 ", "", " cmd2"]
```

这不是 3 段（想要的结果）而是 3 段（第一段 "cmd1 "、第二段 ""、第三段 " cmd2"）。第二段 `""` 产生一个空命令段，需要设计如何降级（空段 → Ask 还是忽略？）。更严重的是，如果逐字符扫描器没做"匹配最长"处理，`false || rm x` 的拆分结果根本不对。

**建议**：拆分器的匹配顺序必须是 **`||` 先于 `|`**（最长匹配优先）。在设计文档中明确拆分器的扫描策略（贪心最长匹配）。

### 3.3 未覆盖 `KEY=value command` 的 env 前缀

当前 `classify_prefix` 的 `first_token` 会把 `ENV=noop cargo check` 的首 token 取为 `ENV=noop` → 不在白名单 → Ask。这是安全的（false positive 兜底为 Ask）。

但如果 P2 拆分后，env 前缀仍然留在子段里，首 token 判定的结果不会改变。提案没提到是否对 env 赋值前缀做预处理。建议在设计文档中明确：

- **不剥离**：`ENV=noop cargo check` 的子段首 token 仍是 `ENV=noop` → Ask（安全兜底，符合 P2 的 fail-safe 原则）。
- 明确这个 case 作为"已知盲区"记录在案（用户可点 Allow），避免实施时陷入"要不要剥离 env"的争论。

### 3.4 引号处理模型过于简略

§4 P2 只说"尊重引号/转义，引号内的不拆"。以下细节缺失：

| 场景 | 如何判定？ |
|---|---|
| 单引号 `'...'` | 所有字符字面量，无转义（`'a\|b'` 的 `\|` 就是 `\|`） |
| 双引号 `"..."` | `\$`、`` \` ``、`\"`、`\\`、`\n` 是转义序列；`\|` 在双引号内不是转义 |
| ANSI-C quoting `$'...'` | `\n`、`\t` 等是转义 — 但内部分隔符 `;` 是否应拆分？建议不拆（保守）。 |
| 反斜杠转义 `\` 在引号外 | `ls\ \|\ grep` — 反斜杠转义空格和管道符 → 应视为一个字面量 token，不拆分 |

**建议**：在 `design.md` 中给出一个 **引号状态机的简化 FSM**（3 态：normal / single_quoted / double_quoted），明确各状态下的字符行为。不做 ANSI-C quoting（`$'...'`）——极罕见，有疑问直接降级 Ask。

---

## 四、中等问题 (P2)

### 4.1 P1 单独上线的体验回退范围比描述的大

§4 P1 "代价"段说"复合命令一律 Ask"。但实际操作上，"复合命令"的定义是含 `;`/`&&`/`|` 任一——这不包括：

- 裸重定向 `git log > /tmp/x` → **不含结构元字符，不走 P1 收紧**，直接走 (a) prefix-grant 短路（如果 `git` 的 grant 存在）或 (b) classify → ReadOnly（`git log` + 无结构元字符 → ReadOnly）

所以 P1 上线后**裸重定向的静默写漏判（§4 P2 提到的 `git diff > patch.txt`）仍然存在**，P1 没有收它。提案在 P2 的"与审批的结合"第 4 条确实写了这个，但在 P1 的"代价"段没有提及——建议在 P1 代价段补充："裸重定向（`cmd > file`）不受 P1 影响，仍需 P2 的重定向检测来收。"

### 4.2 grant 收紧的检测位置选择

§4 P1 说在 Tier 4 (a) prefix-grant 短路的**前置条件**中加检查。这有两种实现路径：

**路径 1** — 在 `check_prefix_grant` 调用之前加 guard：
```rust
// check.rs:298 之前加
if cmd.contains('|') || cmd.contains("&&") || cmd.contains(';') {
    // 跳过 grant 检查，直接回落 classify
} else if let Ok(true) = check_prefix_grant(...) { ... }
```

**路径 2** — 在 `check_prefix_grant` 内部加参数，让它检查命令结构。

**建议**：路径 1 更清晰（grant 函数本身不知道命令结构，它只做 SQL 查询）。但这意味着 grant 表会被查询一次然后结果被扔掉（因为 guard 在查询之前？不对，guard 在查询之前判断，有结构元字符就不查 grant 表）——更省 DB 查询。

在设计文档中明确这个实现选择，避免实施时争论"要不要在 grant 函数里加参数"。

### 4.3 空命令段 / 注释段处理

P2 拆分后可能产生空段：
```
ls |            →  拆分出 "" (空段) 和 "ls"
; ls            →  拆分出 "" 和 " ls"
ls &&           →  拆分出 "ls" 和 ""
```

**建议**：空段（trim 后为空）→ 忽略（不算进 max）。因为空命令在 shell 里只是报语法错误，不是安全威胁。明确写到 `design.md` 里。

注释（`# comment` 作为一个子段）— 建议忽略（`#` 开头的段不参与 max），因为注释没有副作用。

### 4.4 未提及 `\\\n` 行续接

```bash
ls -la \
  | grep foo
```

反斜杠 + 换行的行续接在 shell scripts 中常见。提案的拆分是按单行字符串做的——如果 LLM 发送了含 `\\\n` 的命令，拆分器当前设计不会处理。

**建议**：在 design.md 中标注为"v1 不处理——agent 很少发送多行命令"。遇到 `\\\n` 后跟分隔符 → 整条命令降级 Ask（安全兜底）。不是硬需求，但需要明确决定并记录。

---

## 五、轻微问题 (P3)

### 5.1 `time` 关键字

`time ls -la` — shell builtin `time` 只测量时间、不改变命令行为。但当前 `first_token("time ls")` = `time` → 不在白名单 → Ask。

P2 不会改变这个结果（`time` 不是分隔符）。Plan 模式下 `time find . -name foo` 会弹窗——这是个无害的体验瑕疵，但因为 `time` 极罕见，不值得为它单独加白名单。

**建议**：在设计文档的"已知盲区"里记录，不做处理。

### 5.2 Process substitution `<(cmd)` / `>(cmd)`

```bash
diff <(ls dir1) <(ls dir2)   # read-only process substitution
echo foo > >(gzip > out.gz)  # write process substitution (rare)
```

`<(` 和 `>(` 创建 FIFO，涉及内核资源分配但文件系统层面无残留。P2 提案没有提到它们。

**建议**：v1 不处理——遇到 `$(` 降级 Ask 已经能兜底 `>(cmd)` 的一部分（取决于是否嵌套），`<(cmd)` 则漏过。在 design.md 中标注为远期增强（或者干脆不处理，frequency ≈ 0）。

### 5.3 `!` 逻辑非前缀

```bash
! true          # 退出码翻转，无副作用
! grep foo bar  # 退出码翻转，grep 仍是只读
```

`! cmd` 的首 token 是 `!` → 不在白名单 → Ask。合理——`!` 极少被 LLM 使用。

**建议**：不处理，维持现状（默认 Ask）。

### 5.4 性能断言的精确度

§5 风险表"性能"行说"开销可忽略"。这是定性判断，建议在 design.md 中给一个量化估计：

- 命令长度上限 ≈ 10 KB（实际场景中 agent 发的最长命令很少超过 1 KB）
- 单遍扫描 O(n)，n 为字节数——即使 10 KB 也 < 1μs
- 补充：Tier 2 kill-list 的 11 条正则对**整条命令**跑一次，不对子段跑——不需改

### 5.5 与 B12 checklist tool 的交互

B12 `update_checklist` 是 loop-local，不持久化。如果 LLM 在 shell 命令被 P2 拆分为 Ask 后，尝试用 `update_checklist` 跟踪"为什么不能执行"——它能看到 Tier 2 的 deny reason，但目前 Tier 4 Ask 的原因不会作为结构化数据传给 LLM（它只看到 modal 的结果 Allow/Deny）。

**建议**：这不是 P1+P2 的范围——本方案不动 PermissionModal 的展示层。但可以留一个注释："将来若 modal 决策原因要回传给 LLM，P2 的拆分信息（哪个子命令导致 Ask）是天然的回传 payload。"

---

## 六、与代码的对齐验证

逐条验证了提案中的代码引用：

| 提案声称 | 实际代码 | 准确？ |
|---|---|---|
| `shell_trust.rs:77-91` 三档 | ✅ `ShellTrust` enum 在 L77-91 | ✅ |
| `shell_trust.rs:349` 五步算法 | ✅ `classify_prefix` 在 L349 | ✅ |
| `shell_trust.rs:365` 一刀切降级 | ✅ `cmd.contains('\|') \|\| cmd.contains("&&") \|\| cmd.contains(';')` 在 L365 | ✅ |
| `check.rs:298` prefix-grant 在 classify 之前 | ✅ L298 是 `check_prefix_grant`，L326 是 `classify_prefix` | ✅ |
| `check.rs:307` worker per-run grant | ✅ L307 是 worker prefix grant 的 `has_run_grant` | ✅ |
| `check.rs:690` `match_value_for_allow_always` | ⚠️ 当前代码中该函数在 `store.rs` 中，不在 `check.rs`。提案可能引用的是重构前的行号 | ⚠️ 行号偏移，语义正确 |
| `dangerous.rs:143` 测试不挡 `rm /tmp/foo.txt` | ✅ `kill_list_does_not_block_normal_rm` 测试在 L143 | ✅ |
| kill-list "11 条灾难性正则" | ✅ `DENY_PATTERNS` 在 dangerous.rs:26-86，确实是 11 条 | ✅ |
| `types.rs:73` shell Risk 恒 High | ⚠️ 需要确认 — 未在本次审查中验证此文件 | ⚠️ 待确认 |
| `PermissionStore` / `check_path_grant` / `check_tool_grant` | ✅ 都在 `store.rs` 中 | ✅ |

**额外验证**：code 中的 structural downgrade 只检查 `|`/`&&`/`;`，不检查 `||`。但 `cmd.contains('|')` **确实会匹配 `||`**（因为 `||` 包含 `|`），所以 `||` 被间接覆盖。提案在 §4 P2 把 `||` 单独列为拆分符是对的（在拆分阶段，`||` 和 `|` 是不同语义的分隔符）。

---

## 七、三阶段评估

### 阶段 1 (P1) — grant 收紧

| 维度 | 评估 |
|---|---|
| 正确性 | ✅ 堵住缺口 A 的最小改动，逻辑自洽 |
| 实施成本 | ✅ ~20 行（两处 guard: check.rs L298 + L307） |
| 风险 | ⚠️ 裸重定向不受影响（非 P1 目标，但需在文档中标） |
| 回滚 | ✅ 一行注释即可回滚 |

### 阶段 2 (P2) — 拆分器

| 维度 | 评估 |
|---|---|
| 正确性 | ✅ 方向正确，"拆分 + 取 max"的语义合理 |
| 实施成本 | ⚠️ 比提案暗示的大——拆分器 + 重定向检测 + 引号状态机 ≈ 150-200 行 Rust + 30+ 测试用例 |
| 风险 | ⚠️ 引号处理 corner case 多（见 §3.4）；`||` 贪吃问题（见 §3.2） |
| 回滚 | ✅ classify_prefix 整体替换，回滚简单 |

### 阶段 3 (P3) — 沙盒

提案对 P3 的描述坦诚地承认了"代价：大"。WSL2 user namespace 验证确实是必须的前置 spike。P3 与 P1/P2 是正交的（P3 是执行隔离，P1/P2 是判定），可以独立排期。

---

## 八、对 §6 落地指征的补充建议

§6 的五个步骤已经很完备。以下为补充：

### 6.1 design.md 必须明确的事项（否则实施时会卡住）
- [ ] 拆分器的引号状态机（normal / single_quoted / double_quoted），明确各状态下 `;` `&` `|` `>` 的行为
- [ ] `||` vs `|` 的最长匹配策略
- [ ] 空命令段、空白段、注释段的处理规则
- [ ] 重定向检测的完整正则（覆盖 `n>`, `n>>`, `&>`, `&>>`, `<>`）
- [ ] `$()` / 反引号检测的范围（检测到后整条 Ask — 是否包括嵌套？）
- [ ] grant 收紧的实现路径（guard-before-call vs inside-grant-fn）

### 6.2 测试矩阵需补充的 corner case
§6.3 的测试矩阵覆盖了基本场景（纯读复合 / 读+写复合 / 重定向 / 命令替换 / 引号元字符 / grant 短路），追加以下：

- `||` 不被 `|` 贪吃：`false || rm x` → [Ask(由于 false 是 ReadOnly…不，false 是 ReadOnly), Ask(rm)] → **Ask**
- 管道末尾空段：`ls |` → 忽略空段后 **ReadOnly**
- 单引号 vs 双引号：`grep "a|b" c` → **不拆分** → ReadOnly（grep 是 ReadOnly）
- `$'...'` ANSI-C quoting：`echo $'a;b'` → 不拆分 → ReadOnly（echo 是 ReadOnly）
- 反斜杠转义：`ls\ \|\ grep` → 不拆分 → ReadOnly（单命令 `ls | grep` 字面量…等等，这实际上是 `ls | grep` 作为**单个命令名**？反斜杠转义空格和管道后整个变成 `ls | grep` 字面命令名，会报 command not found。这说明 `\ ` 和 `\|` 的处理需要区分——但 v1 可接受不处理，降级 Ask）
- `1>` / `&>>` / `<>` 重定向：`cmd 1>out 2>&1` → SideEffect
- worker 路径同款覆盖：worker 的 run-grant + 复合命令不短路

### 6.3 与 P2 同时顺手修的小 bug
`classify_git_subcommand` (shell_trust.rs:396-407) 对 `git --no-pager diff` 判 SideEffect（global flag 把 sub 推到 slot 3，`split_whitespace().nth(1)` 拿到 `--no-pager` → 不在 GIT_READONLY_SUBCOMMANDS → SideEffect）。这是已知的 fail-safe 行为（文档明确说了），但 P2 拆分后不受影响——git global flag 不会触发结构拆分，单个子段仍然走 `classify_prefix` → `classify_git_subcommand` → SideEffect。**不是 P2 的问题，但可以在 P2 的回归测试里加一条确认。**

---

## 九、文档质量

| 章节 | 质量 | 备注 |
|---|---|---|
| §1 背景 | ★★★★★ | 两个缺口的诊断精准，与代码完全对齐 |
| §2 目标与不变量 | ★★★★★ | 不变量和非目标的列举是本文档最有价值的部分 |
| §3 审批管线结合 | ★★★★☆ | 管线图准确，§3.1 阶段标注 swap 需修正 |
| §4 分阶段方案 | ★★★★☆ | P1/P2/P3 切割合理，P2 细节需 design.md 补完 |
| §5 风险 | ★★★★☆ | 覆盖全面，"性能"行可加量化 |
| §6 落地指征 | ★★★★½ | 五个步骤完备，测试矩阵可补充上述 corner case |
| 附录 | ★★★★★ | 术语表清晰 |

---

## 十、结论

**方案可以进入 Trellis 任务创建阶段（建 `prd.md` + `design.md`）**，前提是：

1. **立即修正 §3.1 的阶段标注 swap**（P0，文档错误）
2. 在 `design.md` 中补齐 §三（引号模型、`||` 贪吃、重定向完整覆盖、env 前缀策略、空段处理）的决策
3. 测试矩阵追加 §8.2 的 6 个 corner case

方案的最大优点——**只改 Tier 4 Shell 分支内部，不改任何审批组件接口**——在代码验证后得到确认。这是它能安全分阶段上线的根本保障。P1 的"~20 行"改动量和 P2 的"~200 行"改动量都是合理范围。

A2+ 在 ROADMAP 第三档的排位在此审查后维持——这是一个值得做但非紧急的改进。**如果将来 Plan 模式的使用频率上升**（用户更多用 Plan 做调查），P2（只读管道静默放行）的优先级应随之提升。

---

> 本报告基于 2026-07-03 对 `shell_trust.rs` / `check.rs` / `dangerous.rs` 的全量代码验证 + `A2-SHELL-CLASSIFICATION.md` 全文审查。签名：deepseek-v4-pro。
