# Design — C2 证据链:群聊结构化 summary

> 三项用户决策(Q1 schema 结构化 / Q2 只标注不修改 / Q3 全消费面)见 prd.md「已定决策」。本文件只记技术方案。

## 1. 数据结构(单一事实源)

新模块 `app/src-tauri/src/agent/discussion_detail.rs`:serde 类型 + 宽容解析 + 锚点校验纯核。类型被工具层、编排器、DB 行、转录渲染共用;TS/JS 侧各自镜像类型(不生成代码,手写同形 interface,与 `chat.types.ts` 现行惯例一致)。

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]          // 枚举值小写:sessions 域 wire 惯例
pub struct DiscussionDetail {
    #[serde(default)] pub conclusions: Vec<Conclusion>,
    #[serde(default)] pub open_questions: Vec<String>,
}

pub struct Conclusion {
    pub claim: String,
    #[serde(default)] pub anchors: Vec<Anchor>,
    #[serde(default)] pub stance: Stance,    // 缺省 inferred
}

pub struct Anchor {
    pub path: String,
    #[serde(default)] pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub check: Option<AnchorCheck>,          // 校验回填;None = 未校验(live 期)
}

pub enum Stance { Verified, Inferred, Disputed }       // wire: "verified"|"inferred"|"disputed"
pub enum AnchorCheck { Ok, NotFound, LineOutOfRange, OutsideRoot, Unvalidated }
```

- 容错:`#[serde(default)]` + `deny_unknown_fields` **不用**(未知字段忽略);`claim` 空串的条目解析时丢弃;整体解析失败 → `None`(不阻塞收官)。
- anchor 允许无 `line`(文件级锚点,校验只查存在性)。

## 2. 生产端改造

### 2.1 工具 schema(`tools/end_discussion.rs`)

`definition()` 增两个可选属性(手写 JSON Schema,与现风格一致):

```
conclusions: array of { claim: string, anchors?: array of { path: string, line?: integer },
                         stance?: "verified"|"inferred"|"disputed" }
open_questions: array of string
```

`execute_intercept`:从 `input` 取 `conclusions`/`open_questions` 子树 → `serde_json::from_value::<DiscussionDetail>` 宽容解析(失败 warn log + None)→ `st.end_detail = Some(detail)`。返回值不变(tool_result 仍是 summary 叙事——卡与转录的既有数据源不动)。

### 2.2 状态与编排器

- `GroupChatTurnState`(`tools/nominate_speaker.rs`)增 `end_detail: Option<DiscussionDetail>`;`group_chat_loop.rs:430` 初始化处同步。
- 退出块(`group_chat_loop.rs:1086` 一带):`st.end_detail.take()` → 校验(§3)→ `serde_json::to_string` → 传给 finalize。serialize 失败 → warn + None(不 fail)。
- budget 硬停 / preempt 兜底立断等 `end_summary = None` 路径:end_detail 同为 None,行为一致。

### 2.3 prompt 教学(`agent/group_chat_prompts.rs`)

- `moderator_system_prompt` 第 3 条扩写:end_discussion 时**总是**给结构化参数——每条结论一句话 claim;亲自读过代码/证据的挂 anchor(path:line)标 verified;推理未亲证标 inferred 不挂锚点;未达共识标 disputed;未决进 open_questions;`summary` 保留为叙事串联。教学保持紧凑(≤15 行,moderator system prompt 预算敏感)。
- `moderator_wrapup_instruction` / `moderator_resume_instruction`:补一句同款契约(wrap-up 时结构化同样适用;resume 已收官直接 end 的路径亦然)。
- 回归锚点:`tests_group_chat_prompts` 加断言(moderator prompt 含 conclusions/stance 教学;wrapup 指令含同款)。

## 3. 锚点校验(`discussion_detail.rs` 内纯核 + 薄 fs 层)

```rust
pub fn validate_anchors(detail: &mut DiscussionDetail, project_root: Option<&Path>)
```

- `project_root = None` → 所有 anchor.check = Some(Unvalidated)。
- 每 anchor 独立处理(一条失败不影响其他):
  1. `Path::new(&path)`:绝对路径或含 `..` → `std::fs::canonicalize` 语义判定;与 root 拼接后 canonicalize 失败(root 不存在等)→ 全部 Unvalidated。
  2. canonical 结果不在 root 前缀下 → `OutsideRoot`,**不读文件**(编排器直读绕过沙盒模型,root 外一律零 fs 访问;目录穿越/绝对路径自然落此臂)。
  3. 在 root 内:metadata 不存在或 is_dir → `NotFound`。
  4. 存在且 anchor 带 line:BufReader 流式数行,**16 MiB 字节帽**(超帽 → Unvalidated;正常源文件不会触);line > 行数 → `LineOutOfRange`,否则 `Ok`。无 line → `Ok`(仅存在性)。
- 校验函数拆两层:`validate_anchor(path, line, root, &read_fn)` 纯核(注入读,单测用 fixture 字符串)+ `validate_anchors` 薄壳(真 fs)。IO 错误 → Unvalidated,永不 Err。

## 4. 落库与行模型

- 迁移(`db/migrations/schema.rs`,紧挨 discussion_summary 先例):`add_session_column_if_missing(pool, "discussion_detail", "TEXT")`。
- `finalize_group_chat_lifecycle` 增参 `discussion_detail: Option<&str>`,SQL 增 `discussion_detail = COALESCE(?, discussion_detail)`(与 summary 同 COALESCE 语义:兜底立断无 detail 不清掉已有值)。
- `clear_group_chat_lifecycle`(:919-933)增 `discussion_detail = NULL`——**复用场重置漏清 = 续跑场带上场断证结论,专项断言锁定**。
- `SessionRow`(`db/types.rs` + `session_crud.rs:253` SELECT + try_get)增 `discussion_detail: Option<String>`(JSON 文本,与 metadata 同形态,消费方解析)。
- 命令层(`commands/sessions.rs` load_session 等 SessionRow 透出处)零改动——行结构直通 wire。

## 5. 消费面

### 5.1 GUI

**类型与 store**(`chat.types.ts` / `streamEvents.ts` / `streamRehydrate.ts`):

```ts
export interface DiscussionDetail { conclusions: DiscussionConclusion[]; open_questions: string[] }
export interface DiscussionConclusion { claim: string; anchors: DiscussionAnchor[]; stance: "verified"|"inferred"|"disputed" }
export interface DiscussionAnchor { path: string; line?: number|null; check?: "ok"|"not_found"|"line_out_of_range"|"outside_root"|"unvalidated" }
```

- 行类型两处(:584 LoadedSession session / :735 SessionSummary)加 `discussion_detail?: string | null`。
- `streamEvents.ts:1466-1476` 受控合并块加 `if ("discussion_detail" in loaded.session)` 同款;`streamRehydrate.ts:132` 同步。

**收官卡**(`DiscussionSummaryCard.vue`):

- 双通道:live 期 `computed` 解析 `props.call.input`(conclusions/open_questions)即时渲染;收官后 store 的 `discussion_detail`(带 check)按 `path+line` 匹配叠加核验记号。store 取数:组件内 `useChatStore()` + 当前 session(MessageItem 渲染上下文有 session 归属,经 provide/inject 或 prop 下传——实现期取侵入最小者,倾向 prop:MessageItem 已知 session)。
- 渲染:结构化区(stance 徽章 ✅/💭/⚖ + claim + 锚点行 `path:line` + check 记号 ✓/⚠/·)+ 分隔 + summary 叙事 markdown(现行管线不动);`open_questions` 列表段。
- check 记号语义:`ok`→✓;`not_found`/`line_out_of_range`/`outside_root`→⚠+title 说明;`unvalidated`/无 check→·(未校验,不警示)。
- 兜底:`input` 无 conclusions(旧剧本/朴素收官/rehydrate 旧场)→ 现行纯文本渲染,DOM/testid 不变;`input.conclusions` 空数组但 summary 有文 → 同兜底。
- vitest:结构化渲染(四形态)、check 叠加、无结构兜底三组。

### 5.2 MCP(`scripts/group-chat-mcp.mjs`)

- `discussion_result` 输出对象增 `detail`:`loaded.session.discussion_detail` 非空时 `JSON.parse`(坏 JSON → `detail: null` + `detail_warning`,不炸——json_valid 双红线同源精神);否则 null。
- 入参 schema 零改动 → wire 预算锁 3200 不重锁(锁输入;输出不占)。`group-chat-mcp.test.mjs` 加 result detail 映射纯逻辑用例(含坏 JSON 降级)。

### 5.3 双转录导出器

- **Rust**(`agent/group_chat_transcript.rs`):`TranscriptRenderArgs` 增 `discussion_detail: Option<&DiscussionDetail>`(调用点 `group_chat_loop.rs:1131` 手里有已校验的 typed detail,直接传引用,不二次解析);`render_scheduled_transcript` 在 `## discussion_summary` 后渲染 `## conclusions`(`- [verified] claim — path:line ✓` / ⚠ 记号)+ `## open_questions`;None 或空 conclusions 省略节。单测:tests 文件内已有 render 断言处(:374 一带)扩。
- **JS**(`scripts/group-chat-run.mjs` `renderTranscript`):`session.discussion_detail` JSON → 同款节(与 Rust 逐字同形没必要,结构同形即可);坏 JSON 省略 + warning 行(既有 summary 缺失警告同模式)。`group-chat-run.test.mjs` 加渲染用例。MCP `ensureTranscript` 复用此函数,自动受益。

## 6. 兼容与迁移

- 列 additive(旧库 `add_session_column_if_missing` 幂等);行/工具/prompt 全部缺省兼容;旧场 detail NULL 全消费面文本兜底。
- DAEMON-API.md:§4 SessionRow 字段表加 `discussion_detail`;§6 `discussion_result` 输出示例加 detail;转录头部说明补 conclusions 节。
- 收官文档动作:ROADMAP §6 依赖矩阵 C2.1 行划掉;BUGLIST-group-chat 无涉(新能力非缺陷)。

## 7. 权衡记录

- **detail 存 JSON 文本而非拆表**:读路径一次 `load_session` 直达,零 JOIN;写路径单点 finalize;讨论库 LIKE 检索维持扫 summary 文本(detail 的 claim 若要进检索,后继讨论库任务再议,不进本任务)。
- **校验在编排器直读 root 内文件(不走工具沙盒)**:读 only、root 前缀 canonicalize 收紧、16MiB 帽;沙盒模型为工具执行设计,编排器收束路径无工具语境——与 D1 的 project_root 注入同为编排器受信读。
- **tool_result 仍是 summary 文本**:卡/转录的既有信封数据源不动,结构化走 input + 行双通道——避免动 ChatEvent/wire 事件面(影响面远大于收益)。
- **TS 手写镜像类型**:仓库无 codegen 管线,metadata/token_budget 等先例均手写同形;此处字段少且稳定。

## 8. 回滚

- 单列 additive + 全可选参数:回滚 = 不再生产 detail(旧消费面立即兜底文本),库中残留 detail 无消费方、无破坏。GUI/JS 消费端可独立回退。
