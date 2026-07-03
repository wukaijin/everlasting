# Design — A2+ P1+P2:grant 短路收紧 + classify 复合命令拆分取 max

> **prd**:[`prd.md`](./prd.md) · **source**:[`docs/A2-SHELL-CLASSIFICATION.md`](../../../docs/A2-SHELL-CLASSIFICATION.md) §3-§4
>
> 本文档定义技术设计:函数边界、数据流、状态机、兼容性、测试设计、回滚 shape。执行步骤见 `implement.md`。

## 1. 落点与作用面

全部改动集中在两个文件,Tier 4 Shell 分支内部:

| 文件 | 现状 | 本任务改造 |
|---|---|---|
| `shell_trust.rs` | `classify_prefix`(L349)单条 first-token 分类 + L365 一刀切结构降级 | 入口重写:命令替换检测 → 拆段 → 每段 `classify_single` → 取 max;新增 `detect_write_redirect` / `split_top_level` / `has_command_substitution` / `has_structural_metachar` |
| `check.rs` | Tier 4 Shell (a) prefix-grant 短路(L298)+ worker run-grant(L307) | 两处短路前加 `has_structural_metachar` 前置 |

**零改动**:Tier 1/2/2.5/3/5/6、`dangerous.rs`、`shell.rs` 执行层、grant 表 schema(`match_value_for_allow_always` L690)、Mode 枚举、AuditKind、PermissionModal、前端。

## 2. 数据流(改造后)

```
check.rs Tier 4 Shell 分支(cmd)
  ├─ has_structural_metachar(cmd)?  ──no──▶ (a) prefix-grant / worker run-grant 短路照常 → Allow
  │       yes                              │
  │       ▼ (跳过两处短路,强制回落 classify) ▼ 短路未命中
  └─ classify_prefix(cmd) ──────────────▶ ReadOnly/SideEffect/Ask → Mode 映射(L326-351 不动)
        │
        ├─ first_token(cmd).is_empty()        → Ask
        ├─ has_command_substitution(cmd)      → Ask     // $() / 反引号,fail-safe
        ├─ segments = split_top_level(cmd)              // 顶层 ;/&&/||/|,引号感知
        └─ segments.map(classify_single).reduce(max_of)
                        │
                        ▼
          classify_single(seg):
            tier = first_token_classify(seg)            // 复用现有步骤 3-5(git 子命令 + 白名单 + 默认 Ask)
            if detect_write_redirect(seg) { tier = max_of(tier, SideEffect) }
            tier
```

## 3. 函数边界(新增/改造,均在 shell_trust.rs 除非注明)

### 3.1 `has_structural_metachar(cmd: &str) -> bool` —— grant 短路前置

`cmd.contains('|') || cmd.contains("&&") || cmd.contains(';')`(与现有 L365 结构检测**同源**,v1 不引号感知)。

- **用途**:check.rs 两处短路(L298 / L307)的前置 —— true 则跳过短路。
- **为什么 v1 不引号感知**:grant 短路跳过后回落 classify,拆分器会重新精准判定。即使 `echo "a;b"` 被误判有结构元字符 → 跳过短路 → classify 拆分器发现引号内不拆 → 单段 echo → ReadOnly。结果仍正确,只是没享受短路(false positive 安全)。
- 导出 `pub(crate)` 供 check.rs 用。

### 3.2 `has_command_substitution(cmd: &str) -> bool`

`cmd.contains("$(") || cmd.contains('`'`)`。

- fail-safe:**不看引号**(`'$()'` 单引号内字面也判 true → Ask,用户可放行)。
- 命中则整条降级 Ask,**不进拆分器**(拆分器因此不必处理 `$()` 嵌套,状态机简化)。
- `$var`(变量展开)不触发——静态不可知,归"静态盲区",v1 不专门处理(靠 P3 兜底)。

### 3.3 `split_top_level(cmd: &str) -> Vec<&str>` —— 拆分器(核心复杂度)

按顶层 `;` / `&&` / `||` / `|` 拆段,返回各段(借用 cmd,trim 后)。**引号/转义感知**:引号内的元字符不拆。

**前置**:已通过 `has_command_substitution` 过滤(无 `$()`/反引号),故状态机只需 4 态:

| 状态 | 触发 | 转移 / 动作 |
|---|---|---|
| `Normal` | `'` | → `Single` |
| `Normal` | `"` | → `Double` |
| `Normal` | `\` | → `Escaped`(消耗下一字符) |
| `Normal` | `;` | **拆分点** |
| `Normal` | `&` 且下一字符 `&` | **拆分点**(`&&`,消耗两字符) |
| `Normal` | `\|` 且下一字符 `\|` | **拆分点**(`\|\|`) |
| `Normal` | `\|`(单) | **拆分点**(管道) |
| `Single` | `'` | → `Normal`(单引号内一切字面,含 `\`) |
| `Single` | 其他 | 不拆(留在段内) |
| `Double` | `"` | → `Normal` |
| `Double` | `\` 且下一字符 ∈ {`$`,`` ` ``,`"`,`\`,换行} | 转义,消耗下一字符 |
| `Double` | 其他 | 不拆 |
| `Escaped` | 任意 | → `Normal`(消耗该字符) |

- 空段(如 `cmd ;; cmd`)跳过不产出。
- `Vec<&str>` 借用 cmd;若需 trim 则返回 `&str` 切片本身就已可 trim。

### 3.4 `classify_single(seg: &str) -> ShellTrust`

单段(无顶层元字符)判定。**复用现有 `classify_prefix` 步骤 3-5**,加 `>` 检测叠加:

1. `first = first_token(seg)`(复用 L413)
2. `first == "git"` → `classify_git_subcommand(seg)`(复用 L396)
3. `READ_ONLY_WHITELIST.contains(first)` → ReadOnly
4. `SIDE_EFFECT_WHITELIST.contains(first)` → SideEffect
5. 否则 Ask
6. **叠加**:`if detect_write_redirect(seg) { tier = max_of(tier, SideEffect) }`

即把现有 `classify_prefix` L369-386 的逻辑提取为 `classify_single`,加步骤 6。

### 3.5 `detect_write_redirect(seg: &str) -> bool` —— per-segment `>` 检测

seg 含"重定向到文件" → true。**per-segment**(被 `classify_single` 调用)。

扫描非引号内、非转义的 `>`,按后续 token 判定:

| 形态 | 写文件? | 例 |
|---|---|---|
| `>file` / `> file` | ✅ | `git diff > x` |
| `>>file`(追加) | ✅ | `echo hi >> log` |
| `&>file` / `&>>`(bash 整体重定向) | ✅ | `cmd &> f` |
| `[N]>file`(如 `2>err`) | ✅ | `make 2>err.log` |
| `>&N` / `[N]>&M`(fd 复制) | ❌ | `cmd 2>&1`、`>&2` |
| `<` / `<<` / `<<<`(输入) | ❌ | `cat < /etc/hostname` |

判定要点:遇 `>`,看 `&` 的位置——`>&<数字>` 是 fd 复制(不升),`&>`(`&` 在 `>` 前)是 bash 整体重定向(升)。`<` 方向一律不升。

- v1 单遍扫描器实现;corner case(`2>&1 | head`、`<<<`、`&>`)靠测试矩阵锁。
- 复用引号/转义感知(与拆分器同套规则,可抽 `fn char_is_meta_aware` 共用扫描骨架——v1 可先各扫各的,重复 acceptable)。

### 3.6 `ShellTrust` 偏序

加 `fn severity(self) -> u8`(`ReadOnly=0, SideEffect=1, Ask=2`)+ `fn max_of(a, b) -> ShellTrust`。取 max = `severity` 大者。

- 不 derive `Ord`(避免改类型 trait 表面 + 序列化/反序列化风险);`severity()` 内敛。
- enum 变体顺序(L77-91 `ReadOnly, SideEffect, Ask`)正好升序,仅作交叉校验。

### 3.7 `classify_prefix` 入口重写(L349)

```
pub fn classify_prefix(cmd) -> ShellTrust:
    if first_token(cmd).is_empty() → Ask              // 保留 L350-354
    if has_command_substitution(cmd) → Ask            // 新增
    let segs = split_top_level(cmd)                   // 新增(无顶层元字符时返回 [cmd])
    segs.iter().map(classify_single).reduce(max_of).unwrap_or(Ask)
```

- **删除** L356-367 一刀切结构降级(被拆分器取代)。
- 现有 `first_token` / `classify_git_subcommand` / 三张表(READ_ONLY / SIDE_EFFECT / GIT_READONLY_SUBCOMMANDS)**零改动**。

### 3.8 grant 短路前置接入(check.rs L298 / L307-318)

两处短路各包一层:
```rust
if !super::shell_trust::has_structural_metachar(cmd) {
    if let Ok(true) = check_prefix_grant(...).await { ... return Decision::Allow; }
}
// 同理 worker run-grant
```
`has_structural_metachar` 为 true 时跳过短路,直接进 (b) classify。

## 4. 兼容性与不变量保持(对应 prd Constraints)

| 不变量 | 保持方式 |
|---|---|
| Tier 2 kill-list 前置 | 不动 `dangerous.rs`;Tier 2 在 Tier 4 前,本任务零影响 |
| Yolo bypass 整层 | 不动 check.rs 顶部 Yolo 短路(L187) |
| Mode 三档语义 | classify_prefix 仍只产三档之一;Mode 映射(L326-351)零改动 |
| grant schema 三 match_kind | `match_value_for_allow_always`(L690)零改动;只改 check 端短路前置 |
| 17 AuditKind | 不加新 Kind |
| 执行层加固 | 不动 `shell.rs` |
| "不确定就 Ask" | 命令替换/空段/拆分失败一律 Ask |

## 5. 测试设计

### 复用(应保持绿)
- 现有 first-token / git 子命令 / path 前缀剥离 / 三表 size+overlap 测试 → 走 `classify_single`,行为不变。

### 现有结构降级测试 —— **断言需重判**(P2 精化的预期变化)

| 测试 | 输入 | 旧断言 | 新断言 | 原因 |
|---|---|---|---|---|
| `classify_pipe_downgrades_to_ask` | `ls \| grep foo` | Ask | **ReadOnly** | 两 ReadOnly 段 |
| 同上 | `git status \| head -5` | Ask | **ReadOnly** | 两 ReadOnly 段 |
| 同上 | `cat x \| head` | Ask | **ReadOnly** | 两 ReadOnly 段 |
| 同上 | `git log \| bash` | Ask | Ask(不变) | bash 段 = Ask |
| `classify_logical_and_downgrades_to_ask` | `git diff && cargo build` | Ask | **SideEffect** | max(ReadOnly, SideEffect) |
| 同上 | `ENV=noop && cargo check` | Ask | **SideEffect** | ENV=noop 段首 token 非 whitelist → Ask? ⚠️ |
| 同上 | `ls && echo done` | Ask | **ReadOnly** | 两 ReadOnly 段 |
| `classify_logical_or_downgrades_to_ask` | `cargo fmt \|\| true` | Ask | **SideEffect** | max(SideEffect, ReadOnly) |
| 同上 | `git diff \|\| echo nope` | Ask | **ReadOnly** | 两 ReadOnly |
| `classify_sequence_downgrades_to_ask` | `cd foo; ls` | Ask | Ask(不变) | cd 非 whitelist → Ask 段 |
| 同上 | `echo a; echo b` | Ask | **ReadOnly** | 两 ReadOnly |

> ⚠️ `ENV=noop && cargo check`:`ENV=noop` 段首 token 经 `first_token` 提取——`first_token` 取首个空白 token = `ENV=noop`(整串,因无空白分隔它和 `=`),不在 whitelist → Ask。于是 max(Ask, SideEffect) = **Ask**(不是 SideEffect)。**实现时务必跑测试确认 `first_token("ENV=noop")` 实际取值**,据此定断言。这暴露 first-token 分类器对 `VAR=val cmd` 前缀的盲区——v1 接受(归类"读+Ask 段 → Ask"),不专门处理 env-prefix。

### 新增(回归矩阵,落 prd AC 11 项)
- `split_top_level`:引号内元字符不拆 / `&&`/`||`/`;`/`|` 拆 / 空段跳过 / 转义 / `"$()"` 不进(已被前置拦截)
- `detect_write_redirect`:`>`/`>>`/`&>`/`2>err` 升;`2>&1`/`>&2`/`<`/`<<`/`<<<` 不升
- `has_command_substitution`:`$(echo)` / 反引号 → true;`$var` / `\$(` → false(或 true,fail-safe)
- `classify_prefix` 复合矩阵:`ls \| grep`→ReadOnly;`ls; rm`→Ask;`git diff > x`→SideEffect;`echo $(rm)`→Ask;`echo "a;b"`→ReadOnly
- `tests_check.rs` grant 短路:`ls`+grant→Allow;`ls; rm`+grant→不短路(回落 Ask);worker 路径用 `RunGrantCache` 同款

## 6. 回滚 shape

- **P1 子步**(grant 前置 + `detect_write_redirect`):revert check.rs 两处包裹 + 移除 `detect_write_redirect` 接入即可。
- **P2 子步**(拆分器 + 入口重写):revert `classify_prefix` 入口 + 移除 `split_top_level`/`classify_single`。
- P1+P2 同 PR,实际回滚单元是整个 PR(`git revert <merge>`)。
- **grant 数据无需迁移/回滚**——前置是代码层,grant 表行不受影响,存量授权对单条命令仍生效。

## 7. 风险与权衡(深化方案 §5)

| 风险 | 设计决策 |
|---|---|
| 拆分器引号误判(false negative 漏拆) | `has_structural_metachar` 用 `contains`(更宽)做 grant 前置兜底:即使拆分器漏拆,grant 短路也已跳过;kill-list(Tier 2)兜底灾难性模式 |
| 拆分器引号误判(false positive 误拆) | 段更短 → 可能误把复合判轻;靠测试矩阵 + 引号状态机覆盖 |
| `>` 检测 fd 复制误判(`2>&1` 被升) | 扫描器看 `>&<数字>` 形态;测试矩阵锁 |
| `$()` 在单引号内误伤 | v1 接受 fail-safe(`'$()'` → Ask) |
| `VAR=val cmd` env-prefix 盲区 | v1 接受(该段首 token 非 whitelist → Ask → 整条 Ask);远期 first-token 提取可剥 `VAR=val` 前缀 |
| 现有结构降级测试大面积断言变化 | 集中在 shell_trust.rs 结构降级测试段,按 §5 表逐一重判 |
