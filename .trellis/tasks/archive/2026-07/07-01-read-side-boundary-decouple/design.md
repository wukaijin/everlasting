# Design — read 侧路径边界解耦

> 配套 `prd.md`。本文聚焦技术决策与边界;执行清单见 `implement.md`。

## 1. 改动边界(4 处)

| # | 位置 | 改动 |
|---|---|---|
| D1 | `tools/read_file.rs:120` / `grep.rs:150` / `glob.rs:84` / `list_dir.rs`(对应处) | **删除**各自 `execute()` 内的 `assert_within_root` 硬卡。读不存在文件改走原生 `tokio::fs` IO error(不再返回 "cannot be resolved" / "outside project root")。 |
| D2 | `agent/permissions/check.rs` | Tier 流程插入 deny-list 检查(§3);Tier 4 Path 分支项目外侧插入 allow-list 检查(§4)。 |
| D3 | `agent/permissions/sensitive.rs`(新建) | static `SENSITIVE_PATH_PATTERNS` + `TRUSTED_EXTERNAL_PATTERNS` + 匹配函数,对标 `dangerous.rs`。 |
| D4 | `agent/permissions/types.rs` | `PermissionContext` 加 `worktree_path: PathBuf`;`chat_loop.rs` 构造 ctx 时填入(已有 `current_ctx.worktree_path`)。 |

## 2. 新数据流(check.rs)

```
Tier 2   dangerous(shell kill list)                     — 不变
[Tier 2.5] sensitive_path_check(仅 read 族, §3)          — 新增
   extract path → abs_path
   outside = !is_within_root(ctx.worktree_path, abs_path)
   if outside && deny-list 命中(abs_path):
       → Deny{critical:true} + audit ToolDenied/ToolDeniedYolo   // 含 yolo
Tier 3   Mode(Plan blocks write_file/edit_file)          — 不变
Tier 4   Path 分支(项目外时):                            — 扩 allow-list
   ... 现有 grant / inside(cwd anchor, 不变)逻辑 ...
   项目外:
     if allow-list 命中(abs_path) → Allow + audit ToolAllowed   // 新增,免 ask
     else → ask_path(现状)
Tier 5/6 Allow / Audit                                   — 不变
```

**关键顺序**:deny-list 在 Tier 2.5(yolo bypass 之前)→ yolo 也挡;allow-list 在 Tier 4(yolo 已 bypass,对 yolo 无额外效果,主要服务 edit/plan 免弹窗)。

## 3. deny-list 设计(Tier 2.5)

- **触发条件**:tool ∈ {read_file, grep, glob, list_dir}。项目外路径(lexical)直接查 deny-list;**项目内路径额外 canonicalize 查 symlink 逃逸**(canonicalize 后到项目外且敏感 → deny)—— 恢复原 tool 层 `assert_within_root` 的 symlink 保护(lexical deny-list 单独挡不住"项目内 symlink → `~/.ssh/id_rsa`"攻击链)。项目内真文件(canonicalize 后仍项目内)不挡(Q1.2);canonicalize 失败(不存在)不挡(read 走 IO error)。
- **匹配器**:用 **`globset`**(`Cargo.toml:44` 已在依赖;`GlobSet` 一次编译多 pattern、支持 `**`;sqlite GLOB 不支持 `**`,见 `check.rs:453` 注释)。pattern 与 target 均 lexical(不 canonicalize——read 不存在路径要走 IO error 不该走 deny)。
- **`~` 展开**:pattern 含 `~/` 时用 `dirs::home_dir()`(或 `std::env::var("HOME")`)展开后匹配。
- **`.env.example` 豁免**:**不写** `**/.env.*` 通配,改为**枚举**敏感 `.env` 变体(`**/.env`、`**/.env.local`、`**/.env.production`、`**/.env.staging`、`**/.env.*.local`),`example`/`sample`/`template` 天然不命中。
- **pattern 清单(中等档)**:
  - 私钥/证书:`~/.ssh/**`、`**/*.pem`、`**/*.key`、`**/*.p12`、`**/*.pfx`、`**/*.keystore`
  - 系统密钥:`/etc/shadow`、`/etc/gshadow`
  - 凭证:`**/.env`、`**/.env.local`、`**/.env.production`、`**/.env.staging`、`**/*credentials*`、`**/*secret*`、`~/.aws/credentials`、`~/.netrc`、`~/.npmrc`、`~/.docker/config.json`
- **命中措辞**:返回 `"path blocked: matches sensitive-path deny-list (use shell to inspect manually if needed)"`。**不回显具体 pattern 或路径内容**,避免把"这里有私钥"的元信息喂给 LLM。

## 4. allow-list 设计(Tier 4 项目外)

- **pattern**:`~/.config/everlasting/**`(单条,hardcoded)。
- **位置**:Tier 4 Path 分支,项目外、deny 未命中、`ask_path` 之前。
- **匹配器**:同 deny-list(`globset` + `dirs::home_dir()` 展开)。
- **行为**:`Decision::Allow` + `record_audit(ToolAllowed)`(payload 带 path,C4 审计 UI 可见"项目外受信读取")。

## 5. 双 anchor 方案(避免 write 族回归)

权限层现状:`is_within_root(ctx.cwd, path)` 决定 silent-Allow vs ask(types.rs:135-142 注释说明 cwd 是唯一 anchor)。本 task:

- **inside/outside(决定 ask vs silent-Allow)**:**沿用 `ctx.cwd`,不改** → write 族 ask 行为零回归。
- **deny-list / allow-list 的"项目外"触发**:**用 `ctx.worktree_path`**(项目根)→ 符合"项目内不挡"直觉。

**边界例子**(session cwd = `/usr/local/code/github/everlasting/app`,worktree = `/usr/local/code/github/everlasting`):
- 项目根 `.env` → `is_within_root(cwd)=false`(走 ask)但 `is_within_root(worktree)=true`(deny-list 不触发)→ read 时 ask,用户允许后读到,**deny 不挡**。✓ 符合 Q1.2"项目内不挡"。
- `~/.ssh/id_rsa` → 两 anchor 都 outside → deny-list 命中 → 硬 Deny。✓
- `~/.config/everlasting/commands/x.md` → 两 anchor 都 outside → deny 未命中 → allow-list 命中 → Allow(免 ask)。✓

## 6. grep / glob / list_dir 的 deny 语义(已知 gap)

deny-list 只匹配 **tool 的 path 参数**(read_file 目标、list_dir 目标、grep 搜索根、glob 搜索根):

- `grep "token" ~/repos` → path `~/repos` 不命中 deny → **不挡**,即使结果行里含 `~/repos/x/.env` 内容。
- `glob "**/*.pem" /` → path `/` 不命中 deny → **不挡**,即使 glob pattern 本意在找私钥。

**这两个是已知 gap**(堵住需"结果过滤 / pattern 解析",等同 redaction,见 Out of Scope)。本 task 仅挡"path 参数本身敏感"。记入 PRD 局限 + design 注释。

## 7. 兼容性 / 回归

- **write_file / edit_file**:tool 层 `assert_within_root` 不动;权限层 cwd anchor 不动。**零回归**(AC7)。
- **项目内 read**:行为不变(still silent-Allow / cwd-based ask)。
- **read 不存在文件**:错误从 `cannot be resolved` / `outside project root` 改为 IO error(AC6)。
- **spec**:更新 `.trellis/spec/backend/project-cwd-boundary.md` §5——read 族 tool 层 defense-in-depth 移除;新增 §"敏感路径 deny-list / 受信 allow-list"小节(权限层维度)。
- **worker 路径审计**:deny-list / allow-list 命中走 Tier 2 模式(`record_audit` 写父 `session_audit_events`),与 `dangerous.rs` kill-list 一致(`check.rs:84` 无 `is_worker` 特判)。**不触发 RULE-A-016**(该规则仅约束 Tier 4 `ask_path` worker collapse → sink→transcript;Tier 2.5 硬墙是安全事件,worker 命中也写父审计,符合既有 kill-list 行为)。
- **`PermissionContext` 构造点**(加 `worktree_path` 后 cargo check 强制补齐,共 5 处真构造):production `chat_loop.rs:576`、test helper `tests_common.rs:76`(覆盖全部 agent_loop 集成测试)、`tests_ask.rs:521/618/704`。

## 8. 回滚

- D1(删 assert_within_root)+ D3(sensitive.rs)是核心;回滚 = `git revert`。
- D4(PermissionContext 加字段):回滚需同步移除 `chat_loop.rs` 填入点。
- D2(check.rs 插入点):均为早返回分支,删除即回滚,不影响其他 Tier。

## 9. 待 design-review 确认的开放点

- **✅ 已确认**:`globset 0.4`(`Cargo.toml:44`)+ `dirs 5`(`:61`)均在依赖树。匹配用 `globset::GlobSet`、`~` 展开用 `dirs::home_dir()`。**无需加依赖**。
- **deny 措辞是否提及路径**:`§3` 选了"不回显路径"。若 review 觉得"提示哪个 pattern 命中"更利于调试,可放宽(但安全折损——把"这里有私钥"的元信息喂给 LLM)。
