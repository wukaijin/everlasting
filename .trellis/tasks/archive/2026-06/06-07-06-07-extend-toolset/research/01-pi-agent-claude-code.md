# 调研: claude-code + pi-agent 工具集

> Subagent `a87ca256121d5c909` 输出,2026-06-07
> 完整源报告见 subagent transcript,本文件是蒸馏版

## 关键发现

### claude-code 工具集 (code.claude.com/docs/en/tools)

| 工具 | 必选参数 | 关键行为 |
|------|---------|---------|
| **Read** | `file_path`(绝对) | 支持 `offset` / `limit` / `pages`(PDF);超 token 上限自动分页 + 提示续读 |
| **Write** | `file_path`, `content` | 覆写已存在文件前**必须先 Read** |
| **Edit** | `file_path`, `old_string`, `new_string` | `replace_all?` 可选;**三道强制 check**: read-before-edit + stale-after-read + uniqueness |
| **Bash** | `command`, `description?`, `timeout?` | 默认 2min, max 10min;输出 30K cap,超量落盘 |
| **Glob** | `pattern` | 支持 `**` / `*.{ext}`;按 mtime 排序;**cap 100**;**默认不读 .gitignore** |
| **Grep** | `pattern` | ripgrep 正则;`output_mode`: files_with_matches / content / count;支持 `-n` / `-A` / `-B` / `-C` / `head_limit` / `glob` / `type` / `-i` / `multiline`;**默认遵守 .gitignore** |

### Edit 工具三道强制 check (claude-code)

1. **Read-before-edit**: 当前会话必须读过该文件,自 Read 起文件未被磁盘修改
2. **Match**: `old_string` 必须 exact 存在 (一个空白差异都不行)
3. **Uniqueness**: `old_string` 必须唯一;多匹配 → LLM 要么扩大 context,要么 `replace_all: true`

**等价 read 路径**: Bash `cat`/`head`/`tail`/`sed -n`/`grep`/`egrep`/`fgrep` 单文件无 pipe/redirect,也算 read-before-edit 满足。

### pi-agent 工具集 (pi_agent_rust)

| 工具 | 参数 |
|------|------|
| `read` | `path`, `offset?`, `limit?` |
| `write` | `path`, `content` |
| `edit` | `path`, `old`, `new` (surgical string replacement) |
| `hashline_edit` | LINE#HASH 锚点 (string 不唯一时兜底) |
| `bash` | `command`, `timeout?` (默认 120s) |
| `grep` | `pattern`, `path`, `context?`, `limit?` |
| `find` | `pattern`, `path`, `limit?` (基于 `fd`) |
| `ls` | `path`, `limit?` |

**强制约束**:
- `read` 2000 行 / 1MB cap,返回 continuation hint
- `grep` 每行最长 500 字符 (防 minified 炸内存)
- `bash` 120s 超时 → TERM → 5s grace → KILL + 进程树 walk

### damage-control extension (pi)

- **Dangerous Commands**: 正则 block `rm -rf` / `git reset --hard` / `aws s3 rm` / `DROP DATABASE`
- **Zero Access Paths**: 完全屏蔽 `.env` / `~/.ssh/` / `*.pem`
- **Read-Only Paths**: 系统文件 / lockfile
- **No-Delete Paths**: `.git/` / `Dockerfile` / `README.md`
- **damage-control-continue 模式**: 拦截不中断 turn,把"拒绝原因 + 建议"作为 tool_result 喂回去让 LLM 自调

## 对 Everlasting 的具体建议 (蒸馏)

### 1. edit_file (3 道 check 顺序)

```rust
edit_file {
    path: String,            // 绝对路径
    old_string: String,      // exact match
    new_string: String,      // ≠ old_string
    replace_all: Option<bool>,  // 默认 false
}
```

**check 顺序** (User 已选 claude-code 风格):
1. `ReadGuard.verify_read(path)` → 失败: `"You must read_file <path> first."`
2. `ReadGuard.verify_fresh(path)` (mtime/size 未变) → 失败: `"File <path> has changed on disk since you last read it. Re-read it first."`
3. `old_string` 在文件里 0 次 → 失败: `"old_string not found in <path>. Closest match (line N): ..."` (claude-code 风格,带 0-3 相似行 hint)
4. `old_string` 出现 N>1 次且 `replace_all=false` → 失败: `"old_string appears N times in <path>. Add more context or pass replace_all: true."`

**offset/limit 算 read 过**: `old_string` 出现位置在 `[offset, offset+limit)` 范围内就算 (用户决定)

**Bash 等价 read 路径**: v1 暂不实现,后续 PR (用户已选 offset/limit 宽松路径,但 bash 等价需要 bash parser 工作,scope creep)

### 2. grep (spawn ripgrep)

```rust
grep {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    output_mode: Option<String>,   // "files_with_matches" (default) | "content" | "count"
    case_insensitive: Option<bool>,
    show_line_numbers: Option<bool>,  // 仅 content 有意义
    context: Option<u32>,            // -C (覆盖 -A/-B)
    head_limit: Option<u32>,
}
```

**实现**: `tokio::process::Command::new("rg")` spawn,按 output_mode 选 flag:
- `files_with_matches` → `rg -l`
- `content` → `rg -n`
- `count` → `rg -c`

**line cap**: 抄 pi `GREP_MAX_LINE_LENGTH=500`
**.gitignore**: 默认遵守 (与 claude-code 一致)
**无结果返回**: `"No matches found for pattern <pattern>."`

### 3. glob (cap 100)

```rust
glob {
    pattern: String,            // "src/**/*.rs"
    path: Option<String>,
}
```

**实现**: 用 `globset` crate 或 `fd --glob`,cap 100,按 mtime 倒序
**不强制 .gitignore** (与 claude-code 一致)

### 4. list_dir (非递归)

```rust
list_dir {
    path: String,
    show_hidden: Option<bool>,  // 默认 false
    limit: Option<u32>,         // 默认 500
}
```

**实现**: `tokio::fs::read_dir`,字母排序,目录加 `/` 后缀
**不递归** (递归归 glob)
**hidden 默认 false**

## 借鉴 / 不借鉴

### 抄
- ✅ read-before-edit 三道 check
- ✅ Bash 输出超 30K 落盘 + path + preview (现有 shell.rs head+tail 改)
- ✅ GREP_MAX_LINE_LENGTH=500
- ✅ description 措辞简洁 (一段话,内嵌约束)
- ✅ 错误文案是 plain English action (LLM 才能自纠)

### 暂不抄
- ❌ `hashline_edit` (复杂度高,等 string-replace 真的卡了再上)
- ❌ `MultiEdit` (v1 不做)
- ❌ `NotebookEdit` (无需求)
- ❌ `LSP` (rust-analyzer 集成成本高, BACKLOG)
- ❌ `WebFetch` / `WebSearch` (本任务范围外)
- ❌ damage-control 4 档规则 (本任务范围外, BACKLOG)
- ❌ damage-control-continue 模式 (本任务范围外, BACKLOG)
- ❌ Bash 等价 read 路径 (scope creep,后续 PR)

## 信息源 URL

- https://github.com/Dicklesworthstone/pi_agent_rust
- https://raw.githubusercontent.com/disler/pi-vs-claude-code/main/TOOLS.md
- https://code.claude.com/docs/en/tools
- https://code.claude.com/docs/en/tools#edit-tool-behavior
- https://code.claude.com/docs/en/tools#grep-tool-behavior
- https://code.claude.com/docs/en/tools#glob-tool-behavior
- https://code.claude.com/docs/en/tools#bash-tool-behavior
- https://code.claude.com/docs/en/tools#read-tool-behavior
- https://code.claude.com/docs/en/tools#write-tool-behavior
