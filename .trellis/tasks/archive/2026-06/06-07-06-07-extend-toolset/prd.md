# 扩展编码工具集:edit_file / grep / glob / list_dir

> 步骤 4 前的工具集扩展批次 (4 tool 一起,1-2 周,2026-06-07 起)
> Trellis task: `.trellis/tasks/06-07-06-07-extend-toolset/`

## Goal

Everlasting 当前只有 3 个 tool (read_file / write_file / shell),对"编码 agent"定位偏单薄。本批次为自研 agent 加 4 个编码刚需 tool,使其在编码任务上的体验接近 Claude Code:

- **edit_file** —— str_replace_editor 风格,带 read-before-edit 3 道 check,杜绝 LLM 改过时副本/误伤
- **grep** —— spawn ripgrep,3 种 output_mode (files_with_matches / content / count),代码搜索 token 经济
- **glob** —— 按 pattern 找文件,cap 100 防 context 爆炸
- **list_dir** —— 非递归列目录,字母排序 + 目录加 `/` 后缀,跟 glob 互补

**顺手 2 件 (同批次)**:
- **read_file 返回加 `cat -n` 行号 prefix** —— 跟 edit_file 报错带行号协同,LLM 拿到内容就能定位"第 42 行"
- **Bash 输出超 30K 字符落盘** —— 跟 grep 的 line cap 行为一致,claude-code 风格

参考 [claude-code](https://code.claude.com/docs/en/tools) + [pi_agent_rust](https://github.com/Dicklesworthstone/pi_agent_rust) + OpenHands str_replace_editor 的成熟设计。完整调研沉淀见 [`research/01-pi-agent-claude-code.md`](research/01-pi-agent-claude-code.md) + [`research/02-openhands-aider-cline-opencode.md`](research/02-openhands-aider-cline-opencode.md)。

## What I already know

### 现状 (Step 1 auto-context)
- 现有 tool 模式: `app/src-tauri/src/tools/{read_file,write_file,shell}.rs`,每个 `pub fn definition() -> ToolDef` + `pub async fn execute(input, ctx) -> (String, bool)`
- `tools::builtin_tools()` (`mod.rs:23`) 列出,`tools::execute_tool()` (`mod.rs:63`) 分发
- `ToolContext { project_root, cwd }` 在 `lib.rs:604` per turn 构造,`assert_within_root` 是单源边界
- 测试用 `tempfile::tempdir()` + 标准 assert pattern (~10 个 test 每个 tool)
- `read_file` 已有 50KB head+tail 截断 (`truncate_output`),`shell` 已有 5min 超时 + 截断
- `ToolDef` schema 是 JSON Schema 风格 (`serde_json::json!`)

### 已定决策 (2026-06-07 用户拍板)
- **edit_file 0 匹配**: claude-code 风格直接报错,带 0-3 个最相似行 hint (不自动 strip 重试)
- **read fingerprint 存哪**: Tauri State 进程内 (Mutex<HashMap>),session 切走清
- **offset/limit 算 read 过**: 包含 `old_string` 出现位置就算 (不要求覆盖全文)
- **批次**: 4 tool 一起做,1-2 周

## Requirements (待 brainstorm 收敛)

### R1. `edit_file` 工具
- **Schema** (claude-code 对齐):
  ```rust
  edit_file {
      path: String,            // 必选,绝对路径
      old_string: String,      // 必选,exact match 含空白
      new_string: String,      // 必选,≠ old_string
      replace_all: Option<bool>,  // 默认 false
  }
  ```
- **3 道强制 check** (顺序):
  1. `ReadGuard.verify_read(path)` → 失败: `"You must read_file <path> first."`
  2. `ReadGuard.verify_fresh(path)` (mtime/size 未变) → 失败: `"File <path> has changed on disk since you last read it. Re-read it first."`
  3. **Match**: `old_string` 在文件 0 次 → 报错 + 0-3 相似行 hint + 行号
  4. **Uniqueness**: `old_string` 出现 N>1 次且 `replace_all=false` → 报"出现 N 次"行号列表
- **自动 invalidate**: edit_file 写成功后**自动**调 `ReadGuard.invalidate(path)`,逼 LLM 下次读最新内容
- **No-op 拒绝**: `old_string == new_string` → 报错
- **相对路径**: 跟 read_file 现行一致 —— 接受相对,resolve against `ctx.cwd`
- **边界**: 写入路径必须 `assert_within_root` 过

### R2. `grep` 工具
- **Schema** (claude-code 对齐,简化):
  ```rust
  grep {
      pattern: String,         // ripgrep 正则
      path: Option<String>,    // 默认 ctx.cwd
      glob: Option<String>,    // 文件名限定
      output_mode: Option<String>,   // "files_with_matches" (default) | "content" | "count"
      case_insensitive: Option<bool>,
      show_line_numbers: Option<bool>,  // 仅 content 有意义
      context: Option<u32>,    // -C
      head_limit: Option<u32>,
  }
  ```
- **实现**: `tokio::process::Command::new("rg")` spawn
  - `files_with_matches` → `rg -l`
  - `content` → `rg -n`
  - `count` → `rg -c`
- **强制约束**:
  - 默认遵守 .gitignore (跟 claude-code 一致)
  - 单行最长 500 字符 (抄 pi_agent_rust `GREP_MAX_LINE_LENGTH`)
  - 无结果返回: `"No matches found for pattern <pattern> in <path>."`
- **错误处理**: rg exit code 1 = no match (不算 error),其他非 0 = error

### R3. `glob` 工具
- **Schema**:
  ```rust
  glob {
      pattern: String,         // "src/**/*.rs"
      path: Option<String>,    // 搜索根
  }
  ```
- **实现**: 用 `globset` crate (已加进 `Cargo.toml` dependencies)
- **强制约束**:
  - cap 100 条 (跟 claude-code 一致)
  - 按 mtime 倒序
  - **不强制 .gitignore** (跟 claude-code 一致,跟 grep 行为不同)
  - 超 cap 返回 truncation hint: `"...and N more, narrow your pattern"`
- **路径 resolve**: 跟 read_file 一致,接受相对,resolve against `ctx.cwd`

### R4. `list_dir` 工具
- **Schema** (pi_agent_rust 对齐):
  ```rust
  list_dir {
      path: String,            // 必选,绝对/相对
      show_hidden: Option<bool>,  // 默认 false
      limit: Option<u32>,         // 默认 500
  }
  ```
- **实现**: `tokio::fs::read_dir`,字母排序,目录加 `/` 后缀
- **强制约束**:
  - **不递归** (递归归 glob)
  - hidden 默认 false (避免 `.git/` 灌爆)
  - 超 limit: `"...truncated, N entries hidden"`
- **LLM-facing description**: 强调"Use `glob` for recursive discovery"

### R5. ReadGuard (Tauri State, Session 隔离)
- **数据结构**: `Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>` (session 隔离,切回不用重读)
- **Fingerprint** = `{ mtime: SystemTime, size: u64, content_hash_head: u64 }`
  - `content_hash_head` = 读文件前 8KB 算 xxhash (不读全文,O(1) 快)
- **API**:
  - `record_read(session_id, path)` —— read_file 成功后调
  - `verify_read(session_id, path) -> Result<(), String>` —— edit_file 开头调
  - `verify_fresh(session_id, path) -> Result<(), String>` —— 重新 stat 比对
  - `invalidate(session_id, path)` —— edit_file 写成功后调,逼 LLM 重读
  - `clear_session(session_id)` —— session 删除调
  - (无) `clear_all()` —— 不需要,新 session 自然空
- **集成位置**: `app/src-tauri/src/tools/read_guard.rs` (新文件)
- **注入方式**: Tauri State,`lib.rs` 在构造 ToolContext 同一个地方 manage
- **SessionId 来源**: `ChatMessage.session_id`(已有)

### R6. 错误返回格式 (跟现有一致)
- `(String, bool)` 元组,跟 read_file / write_file 现行一致
- `is_error=true` 时 msg 是 LLM 能 act 的 plain English,带 hint + 行号
- 不引入新结构化错误类型 (避免大改)

### R7. read_file 加 `cat -n` 行号 prefix
- **改动**: 读成功后,**每行**前缀 `\t<line_num>\t`,line_num 从 1 开始
- **示例** (原内容 "hello\nworld"):
  ```
      1	hello
      2	world
  ```
- **截断行为不变**: head/tail 50KB 仍生效,行号连号 (不被截断重置)
- **LLM-facing description 更新**: 加一句 "Output is prefixed with line numbers in `cat -n` format to help you reference specific lines in edit_file"
- **前端渲染**: ChatWindow.vue 已有 markdown code block 渲染,行号 prefix 跟代码一起显示没问题(用户视觉无感,因为代码块原本就 monospace)
- **测试加**: `read_file_line_numbers_format` (3 个 test: 简单文件 / 截断保留行号 / 空行行号连续)

### R8. Bash 输出 30K 字符落盘 (claude-code 风格)
- **改动** `app/src-tauri/src/tools/shell.rs`:
  - 输出 ≤ 30K 字符 → 照旧返回
  - 输出 > 30K 字符 → 存 `<session_cwd>/.everlasting/outputs/<uuid>.txt`,tool_result 返回 `"Output saved to <path> (N bytes). First 1KB preview:\n<preview>"` + 路径
- **目录**: `<session_cwd>/.everlasting/outputs/`(放在 cwd 下,跨设备同步,跟 git 保持隔离)
- **.gitignore 提示**: 文档提示用户在 `<project>/.gitignore` 加 `.everlasting/`
- **进程清理**: session 结束清理 `.everlasting/outputs/`
- **测试加**: `shell_output_truncation` (3 个 test: < 30K 照旧 / > 30K 落盘 + preview / 进程清理)
- **现有 head+tail 截断保留**: 落盘时,tool_result preview 部分仍 head+tail 1KB (避免 preview 再炸)

## Acceptance Criteria (testable)

- [ ] **AC1**: `edit_file` 8 个单元测试全过 (happy + 4 种 read guard 失败 + 0/N 匹配 + no-op + 相对路径)
- [ ] **AC2**: `grep` 6 个单元测试全过 (3 种 output_mode + 无结果 + line cap 截断 + 相对路径)
- [ ] **AC3**: `glob` 5 个单元测试全过 (基本匹配 + cap 100 + mtime 排序 + 隐藏 + 相对路径)
- [ ] **AC4**: `list_dir` 4 个单元测试全过 (基本列 + hidden + limit + 目录加 `/`)
- [ ] **AC5**: ReadGuard 6 个单元测试全过 (record + verify + fresh + invalidate + clear_session + 跨 session 隔离)
- [ ] **AC6**: `read_file` cat -n 行号 prefix 3 个测试全过 (简单 / 截断保留 / 空行连续)
- [ ] **AC7**: `shell` 输出 30K 落盘 3 个测试全过 (< 30K 照旧 / > 30K 落盘 + preview / session 结束清理)
- [ ] **AC8**: e2e "读(cat -n) → 改 → 再读(cat -n)" 流程跑通,1 个 commit
- [ ] **AC9**: `cargo test` 全过 (103 现有 + ~35 新 = ~138)
- [ ] **AC10**: `pnpm build` 干净
- [ ] **AC11**: 6 个 tool 出现在 `builtin_tools()` 列表,前端能见到 description (`SessionList` 不动)
- [ ] **AC12**: docs 更新:
  - `docs/ARCHITECTURE.md` §2.2 ⑩ Tool 执行段: 加 4 个 tool 引用 + ReadGuard 段 + Bash 落盘段
  - `docs/IMPLEMENTATION.md` §3 路线图全貌: 加 1 行 "工具集扩展 (4 tool + ReadGuard + Bash 落盘 + cat -n)" ✅
  - `docs/HANDOFF.md` §2 追加
  - 决策日志 2026-06-07 加一条
  - `.trellis/spec/backend/llm-contract.md` 加 4 个 tool 描述

## Definition of Done (team quality bar)

- [ ] 所有 AC 满足
- [ ] 现有 103 cargo test + ~35 新 test 全过
- [ ] 现有 36 vitest 全过
- [ ] `pnpm build` + `cargo check` 干净
- [ ] docs/ 三处更新 (ARCHITECTURE / IMPLEMENTATION / HANDOFF)
- [ ] 决策日志加 1 条 (2026-06-07 "工具集扩展批次")
- [ ] spec 更新: `.trellis/spec/backend/llm-contract.md` 加 4 个 tool 描述 (Anthropic JSON Schema 必填字段约束)
- [ ] **commit 策略**: 1 个 `feat(tools):` commit 一次性合 (4 tool + ReadGuard + Bash 落盘 + cat -n,用户拍板)
- [ ] **Rollout**: 默认开,不引入 feature flag (本项目无 rollout 机制);回滚: revert 1 commit 即可

## Technical Approach (summary)

- **加 6 个 Rust 文件**: `edit_file.rs` / `grep.rs` / `glob.rs` / `list_dir.rs` / `read_guard.rs` / (内部模块) `shell_output_disk.rs` (Bash 落盘逻辑)
- **改 5 个文件**:
  - `tools/mod.rs` (加 pub mod + builtin_tools 6 个 + execute_tool 4 个 case)
  - `lib.rs` (manage ReadGuard state + per turn 注入 SessionId)
  - `read_file.rs` (调 ReadGuard.record_read + 加 cat -n 行号 prefix)
  - `shell.rs` (调落盘逻辑,30K 阈值,preview 1KB)
  - `Cargo.toml` (加 `globset` + `xxhash-rust`)
- **前端**: 0 改动 (`SessionList` 通用 tool card,新 tool 复用;`cat -n` 前缀在 markdown code block 内自然显示)
- **跨层契约**: 每个 tool 输出格式 (`(String, bool)`) 跟现有完全一致

## Decision (ADR-lite)

**Context**: 现有 3 tool 偏少,编码场景下 LLM 反复 `cat | sed` 浪费 token 且容易改错。需要 4 个编码刚需 tool + 防护层杜绝 LLM 改过时副本。

**Decision** (2026-06-07 用户拍板):
1. edit_file 用 claude-code 风格 str_replace_editor,**0 匹配直接报错**(不自动 strip 重试)
2. ReadGuard 走 Tauri State 进程内 `Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`,session 切回不重读
3. read_file 传 `offset/limit` 只要包含 `old_string` 位置就算 read 过
4. 4 tool 一起做,1-2 周
5. **commit 策略**: 1 个 `feat(tools):` commit 一次性合 (含 Bash 落盘 + cat -n)
6. **顺手加** (同批次): Bash 输出 30K 落盘 + read_file 加 cat -n 行号 prefix
7. **ReadGuard session 隔离**: `HashMap<SessionId, ...>` 而非全局共享

**Consequences**:
- ✅ token 经济 (claude-code 风格 edit 比整文件 write 省 10-100x)
- ✅ 防 LLM 改过时副本 (ReadGuard 3 道 check)
- ✅ 防误伤 (offset/limit 包含 old_string 位置)
- ✅ LLM 拿到 read_file 就能定位"第 42 行"(cat -n prefix)
- ✅ Bash 大输出不丢失(落盘),LLM 拿 path 自己再 read
- ⚠️ ReadGuard 内存: 10 session × 100 path = ~1000 entry,每 entry ~50B = ~50KB,可忽略
- ⚠️ 实施面广 (4 tool + ReadGuard + Bash 落盘 + cat -n),需要单 commit 全过 35 新 test
- 🔮 后续可加: `hashline_edit` (老 string 不唯一时 LINE#HASH 锚点) / `damage-control` (危险路径规则) / `WebFetch` / bash `cat|head|sed` 等价 read 路径

## Out of Scope (本批次不做)

- ❌ `hashline_edit` —— string-replace 解决不了的极端 case 再上
- ❌ `MultiEdit` / `NotebookEdit` —— 无需求
- ❌ `LSP` 集成 (rust-analyzer 等) —— 复杂度高
- ❌ `WebFetch` / `WebSearch` —— 跟编码任务无关
- ❌ Bash `cat`/`head`/`sed -n` 等价 read —— 需要 bash parser,scope creep
- ❌ damage-control 4 档路径规则 (`.env` / `.ssh/` / etc) —— 独立 PR
- ❌ `replace_all: true` 时的 N 行变化 preview —— v1 不做,只成功返字节数
- ❌ Frontend 改造 (`SessionList` tool card 加新 tool 图标 / `cat -n` 行号 prefix 视觉优化) —— 现有 tool card 通用,新 tool 复用
- ❌ read_file 加 `pages` 字段 (PDF 分页) —— 无需求
- ❌ read_file 加 binary 检测 (现在 read_to_string 失败) —— 后续 PR
- ❌ `read_many_files` (一次多文件批量读) —— 后续 PR
- ❌ grep `output_mode=json` (结构化输出) —— LLM 拿 text 就够

## Technical Notes

### 现有约束
- Tauri 2 IPC: multi-word arg 用 camelCase (`{ projectId, initialCwd }`),snake_case 字段在 struct 序列化时保留 (2026-06-07 决策)
- 错误返回 `(content: String, is_error: bool)` 元组,跟 read_file / write_file 完全一致
- 现有 `truncate_output` 模式可直接复用 (`edit_file` 写成功后返回字节数即可,不需要截断)
- 测试用 `tempfile::tempdir()`,跟 read_file 测试模式一致

### 依赖新增
- `globset` (Rust glob 库,2.x 稳定)
- `xxhash-rust` (轻量哈希,O(1) 头部算)
- `tokio::process` (已内置,只需 `use`)

### 调研源
- [research/01-pi-agent-claude-code.md](research/01-pi-agent-claude-code.md) — claude-code + pi_agent_rust 工具集
- [research/02-openhands-aider-cline-opencode.md](research/02-openhands-aider-cline-opencode.md) — OpenHands str_replace_editor 防护逻辑 + 8 测试用例

### 借鉴项目 URL
- https://code.claude.com/docs/en/tools
- https://github.com/Dicklesworthstone/pi_agent_rust
- https://github.com/All-Hands-AI/OpenHands
- https://github.com/Aider-AI/aider
- https://github.com/cline/cline
- https://github.com/sst/opencode
