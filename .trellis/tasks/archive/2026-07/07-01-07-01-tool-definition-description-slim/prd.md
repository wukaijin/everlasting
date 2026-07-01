# PRD: Tool description 顺手清理(删与 schema/返回值重复的冗余)

> **演进**:本任务最初(v1)想做 skill 下沉,实施后回退;v2 想做 1.7K token 精简,分析后发现 ROI 不成立(caching 抹平首请求收益、长会话几乎无感)。当前为 **v3:只删 description 里与 schema 字段 / 返回值 / 代码常量明显重复的措辞**,定位是顺手清理,不追求 token 目标。

## 1. 背景

### 1.1 为什么不做大改
tools 段 21,154 B / ~6044 tokens,其中 schema 8,230 B(39%)是字段契约不能动;description 11,815 B 里大量内容是**行为契约**(timeout 引导、do-NOT-retry、SSRF、remember When-to/Do-NOT)也不能动。深挖后(见 v2 调研):

- **首请求 15% 收益不痛不痒**:11.1K → 9.4K 在 512K window 里占比从 2.2% 降到 1.8%。
- **caching 抹平长会话收益**:tools 段在稳定前缀里,第 2 轮起 cache hit(0.1× 计费),首请求省的 1.7K 长会话实际只剩 ~170 token/轮。
- **v1 skill 下沉有真实伤害**:skill body 常驻、compaction 不保护 L0 列表、漏调 use_skill、worker 引导缺口。已回退。

### 1.2 顺手清理的定位
**判据:某段文字在 description 里说的内容,schema 字段 description / tool 返回值 / 代码常量里已经说过了——属于冗余复述,删了零风险。**

不做任何"重构",不动 schema,不引入新机制,不改 system_prompt。

## 2. 范围

### 2.1 精简原则
1. **只删重复**:schema 里已有的字段说明、返回值自带的状态枚举、代码常量已有的清单。
2. **保留所有行为契约**:timeout 引导、do-NOT-retry、SSRF 说明、read-before-edit、full-replace 约定。
3. **保留 worker 唯一引导**:`remember` 的 When-to/Do-NOT(`general-purpose` worker 看不到 main system_prompt,只能靠 tool description 获得引导——核实见 §3)。
4. **保留预防性引导**:LLM 调用前预知才有价值的内容(如 merge_worker 的 5 条错误清单)。

### 2.2 逐工具清单

| 工具 | desc B | 删什么(= schema/返回值/代码已有) | 留什么 | 预期省 B |
|---|---:|---|---|---:|
| `shell` | 1684 | env 白名单逐项罗列(PATH/HOME/...9 项,代码 `SAFE_ENV_VARS` 有) | timeout 引导、working_directory、find -exec 禁用 | ~180 |
| `run_background_shell` | 1458 | env 白名单(与 shell 重复)、spillover 细节(shell 已说) | max_runtime_ms、"用于超 600s 长命令" | ~250 |
| `shell_status` | 857 | running/completed/killed 三态枚举(返回值自带) | "查后台 shell 状态"、">30KB 存盘" | ~150 |
| `shell_kill` | 531 | session_id handle 格式(schema 已描述) | "SIGKILL 整个进程组"、idempotent | ~80 |
| `remember` | 1588 | 与 schema 8 条字段 description 重复的措辞(scope/tags 含义复述) | **When-to/Do-NOT(必须留,worker 唯一引导)**、kind=pitfall 字段引导 | ~200 |
| `merge_worker` | 1566 | 无(schema 无字段 description,5 条错误清单**保留**——用户判定为预防性引导) | 全部保留,仅润色 | ~30 |
| `discard_worker` | 881 | 重复的"parent branch 不受影响" | do-NOT-retry on already destroyed | ~80 |
| `ask_user_question` | 1035 | Constraints 列表(1..=4 / 2..=4 / ≤12,schema 的 minItems/maxItems/maxLength 已有) | "问用户结构化多选题"、跳过返回 cancelled | ~150 |
| `dispatch_subagent` | ~500 | worker 继承权限机制描述(实现细节) | "派 worker 子代理"、返回 worker_run_id | ~100 |

**预期总收益 ~1,220 B ≈ 350 tokens。** 不追求具体目标,清理到自然收敛即可。

### 2.3 不动的工具(已经够短,或 description 本就是契约)
`read_file`、`write_file`、`edit_file`、`grep`、`glob`、`list_dir`、`web_fetch`、`use_skill`、`update_checklist`。

## 3. 关键约束:remember 的 When-to/Do-NOT 不能删(核实记录)

**`general-purpose` worker 看不到 main system_prompt**:
- worker 的 system prompt 由 `assemble_subagent_prompt(def, task)`(`subagent/mod.rs:498`)构造,**完全替换** parent system_prompt(注释 mod.rs:339-341:"worker does NOT inherit the main system prompt")。
- worker 拿到的是 `SubagentDef.system_prompt` 字面值(`general-purpose` 那段,mod.rs:425-435),**不含** remember 引导。
- 而 `remember` **不在** `STRUCTURALLY_DISABLED`(`mod.rs:580`)——`general-purpose` worker(`tools: vec![]` 继承全工具集)拿得到 remember 工具。
- 结论:worker 调 remember 时,唯一引导来源就是 tool description。**When-to/Do-NOT 要点保留,只压缩与 schema 字段重复的措辞。**

## 4. 验收标准

### 4.1 功能正确性
- [ ] `cargo test --lib` 全部 pass(本机若缺 gdk-pixbuf 编译不过,记 jsonl,CI 跑)
- [ ] `cargo check` pass
- [ ] **不动 input_schema**(逐工具 diff 确认 schema 段未改)
- [ ] **不动 system_prompt.rs**
- [ ] **不创建任何 skill 文件**

### 4.2 不破坏行为契约(self-check)
改后 description 必须仍含:
- [ ] shell:`300000-600000` timeout 引导 + build/install 举例(护栏测试 `definition_documents_timeout_guidance`)+ find -exec 禁用
- [ ] merge_worker / discard_worker:do-NOT-retry
- [ ] remember:When-to / Do-NOT 要点(worker 唯一引导)
- [ ] web_fetch(不动):SSRF 说明

### 4.3 文档 & commit
- [ ] 同步 `/tmp/measure_tools.py` 的 description 字面值,记录新基线
- [ ] `docs/SESSION-FIRST-MESSAGE-INTERFACE.md` 的工具字节表更新(可选,收益小不一定值得改文档)
- [ ] 单 commit:`chore(tools): 删 9 个 tool description 里与 schema/返回值重复的冗余措辞`

## 5. 风险与回滚

- **风险极低**:只删重复信息,不删行为契约。每个删除点都能在 schema/返回值/代码里找到原文。
- **回滚**:单 commit,`git revert` 即可。

## 6. 不做什么

- ❌ 不追求 token 目标(顺手清理,清理到自然收敛)
- ❌ 不创建 skill / 不动 use_skill 机制
- ❌ 不改 input_schema
- ❌ 不改 system_prompt.rs
- ❌ 不改 dispatch_subagent 的 enum 内容
- ❌ 不实现 tool_search
