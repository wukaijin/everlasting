# 调研: OpenHands / Aider / Cline / OpenCode

> Subagent `a547dc989f69b0a32` 输出,2026-06-07
> ⚠️ SECURITY WARNING: subagent 自动模式遇到权限限制,信息略少于子报告 1
> 核心要点仍可用

## OpenHands · str_replace_editor (祖师爷)

Fork 自 `anthropic-quickstarts/computer-use-demo`,5 个 command:
- `view` (只读,支持 view_range 行号)
- `create` (写新文件)
- `str_replace` (old_str → new_str)
- `insert` (在指定行后插入)
- `undo_edit` (撤销最近一次编辑)

**str_replace 防护逻辑**:
- 0 匹配: 自动 strip trailing whitespace 重试一次
- N>1 匹配: 报错时**列出全部行号** (`Found N matches at lines: 3, 17, 42`)
- 强制: new_str != old_str, old_str != "", 绝对路径

**成功消息格式**:
```
The file {path} has been edited. Here's the result of running 'cat -n' on a snippet...
```
返回的 snippet 是 old_str 周围 ~6 行 (前后各 3),给 LLM 下次决策上下文。

## Aider

- 核心 edit 工具叫... 实际上**没有 tool call schema** —— LLM 在自由文本里输出 `SEARCH/REPLACE` code block
- 4 级 fuzzy fallback: 完美匹配 → 行 trim → `...` placeholder → 0.8 阈值的 SequenceMatcher
- 失败时报错里**直接附上文件里最相似的几行** (find_similar_lines)
- `repo_map`: NetworkX + PageRank + 个性化 + 二分搜索 token 预算,生成整仓库结构图

**对 Everlasting**: Aider 风格不直接借鉴 (没有 tool schema,LLM 学习成本高),但 `find_similar_lines` 错误返回思路值得抄 —— 0 匹配时给相似行,LLM 能自纠。

## Cline (vscode-ide)

24 个独立 tool handler,工具集比 claude-code 大:
- `read_file`, `write_to_file`, `apply_diff` (OpenAI 风格 `*** Begin Patch` 协议)
- `replace_in_file` (Aider 风格)
- `execute_command`, `list_files`, `search_files`, `read_directory`
- `ask_followup_question`, `attempt_completion`, `plan_mode_respond`

**工具审批 UX**:
- `yoloMode` / `autoApproveAll` 全局开关
- per-tool + per-path 双重开关
- `local` vs `external`(命令)独立控制
- 失败时用户除 approve/reject,还能**附带文本/图片/附件**作为 feedback 注入下一轮

**apply_diff**: OpenAI 风格 `*** Begin Patch / *** End Patch`,LLM 输出 diff 文本,前端预渲染 + 用户 confirm。

**对 Everlasting**: 工具审批 UX 涉及多步,本任务不抄 (BACKLOG)。`plan_mode_respond` 是 PLAN mode 概念 (路线图 BACKLOG §4.2)。

## OpenCode (sst/opencode, TypeScript + Effect)

**Tool trait 设计**:
```typescript
interface Tool<Parameters, M> {
  Info: { Parameters; M };  // 懒工厂
  Def: 初始化
  ctx.ask() 申请权限
}
```

**edit.ts 9 级 replacer chain** (User 已知;v1 不抄,记 BACKLOG):
- SimpleReplacer
- LineTrimmed
- BlockAnchor
- WhitespaceNormalized
- IndentationFlexible
- EscapeNormalized
- TrimmedBoundary
- ContextAware
- MultiOccurrence

带 Semaphore 文件锁 + `isDisproportionateMatch` 防误伤 (防止 old_str 太短匹配到一大段)。

read/grep/glob 都有 50KB / 2000 行 / 100 匹配的多级截断。

**对 Everlasting**: TypeScript + Effect 不直接借鉴 (我们栈 Rust + tokio),但 `isDisproportionateMatch` 防 old_str 太短误伤 思路可借鉴 (Rust 端加一个 if old_string.len() < 20 { return err(...) } 的轻量检查)。

## 我们的"防护层"具体设计 (3 层 + 8 测试)

### 第 1 层: 参数前置 check
- `old_string` 为空 → 错
- `old_string == new_string` → 错
- `path` 相对路径 → 错 (强制绝对,跟 read_file 现行实现不同,需要先 resolve)

### 第 2 层: read_file 防护
- Tauri State 全局 singleton,`Mutex<HashMap<PathBuf, Fingerprint>>`
- `Fingerprint = { mtime: SystemTime, size: u64, content_hash: u64 }`
- `record_read(path)` 在 read_file 成功后调
- `verify_read(path)` 在 edit_file 开头调,查表
- `verify_fresh(path)` 读当前 mtime/size,跟记录比较
- 切换 session 时清表 (用 current_session_id 跟踪)
- 写成功后**自动失效** read cache (逼 LLM 重新读)

### 第 3 层: str_replace_editor 唯一性
- 0 匹配 → claude-code 风格报错 + 0-3 相似行 hint
- N>1 匹配 → 报"出现 N 次"行号列表
- new_str == old_str → 错 ("no-op edit")

### 8 个测试用例

1. `happy_path` — read + edit + write back 成功
2. `edit_before_read` — 0 read 就 edit,read guard 拦截
3. `edit_after_external_modify` — read 后文件被外部改,mtime/size 校验失败
4. `edit_old_string_not_found` — 0 匹配,报错带 hint
5. `edit_old_string_ambiguous` — N>1 匹配,报行号列表
6. `edit_no_op` — old == new,报错
7. `edit_relative_path` — 拒绝相对路径
8. `edit_writes_new_file_after_read` — 文件先 read 改 0 字节,再 edit 创建内容

## 信息源

- OpenHands: https://github.com/All-Hands-AI/OpenHands (runtime/plugins/str_replace_editor/)
- Aider: https://github.com/Aider-AI/aider (aider/coders/base_coder.py, aider/repo.py)
- Cline: https://github.com/cline/cline (src/core/tools/)
- OpenCode: https://github.com/sst/opencode (packages/opencode/src/tool/)
