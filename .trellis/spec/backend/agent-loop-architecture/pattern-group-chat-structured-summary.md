# Pattern: 群聊结构化收官结论(C2 证据链:end_discussion 结构化参数 + 锚点后校验)

> 来源:任务 `.trellis/tasks/09-09-gc-c2-evidence-summary/`(群聊内部改进线 C2,
> GROUP-CHAT-API-ROADMAP §6 依赖矩阵收官项)。消费方:群聊编排器、
> `end_discussion` 工具层、DB 生命周期列、GUI 收官卡、MCP `discussion_result`、
> M1/定时双转录导出器。契约文档:[DAEMON-API §4](../../../../docs/DAEMON-API.md)。

## 1. Scope / Trigger

- Trigger:跨层契约变更(DB 新列 + 工具 schema 扩展 + wire 字段 + 四消费面)。
- 目标:群聊收官结论**可信度可分层、证据可核验**——每条结论挂 file:line 锚点 +
  实证/推测/争议自报标注,机制层对锚点做后校验;外部消费方(MCP/GUI/转录)机器
  可读区分实证与推测。

## 2. Signatures

- 工具:`end_discussion({summary?, conclusions?, open_questions?})`——全参数可选
  (additive,只发 summary 的旧剧本零变更)。`conclusions: [{claim(必填),
  anchors: [{path(必填), line?}], stance?: verified|inferred|disputed}]`;
  stance 缺省 `inferred`(保守缺省:未声明不享受实证待遇)。
- DB:`sessions.discussion_detail TEXT`(JSON,additive 列,先例
  `add_session_column_if_missing`);JSON 键**snake_case**(sessions 域惯例,
  区别于 providers 域 camelCase——两域并存是既有事实,不是不一致)。
- 生命周期(与 stop_reason/discussion_summary 完全同构):
  `clear_group_chat_lifecycle`(编排启动,三列全清 NULL)→
  `finalize_group_chat_lifecycle(pool, sid, stop_reason, summary, detail)`
  (编排退出,`COALESCE` 语义——None 不清既有值)。

## 3. Contracts

- 类型单源:Rust `agent/discussion_detail.rs`(`DiscussionDetail` serde 类型 +
  `parse_from_input` 宽容解析 + `validate_anchors`);TS `chat.types.ts`
  `DiscussionDetail` 手写同形镜像(仓库无 codegen,与 ParticipantConfig 同惯例);
  JS 脚本鸭子类型(逐字段防御)。
- 解析宽容性:逐条结论独立解析,**坏条目丢弃不连坐**(`{"claim":"ok","stance":
  "bogus"}` 整条丢,不是修成 inferred);全部条目被丢弃 = 无结构化产物
  (`None`),收官路径**永不**因格式问题失败。
- 校验语义(用户裁定 2026-09-09,**只标注不修改**):锚点附 `check` 五值
  (`ok`/`not_found`/`line_out_of_range`/`outside_root`/`unvalidated`),
  不改写 claim 与 stance——机制层提供事实,断证的语义解释留给消费方。
  自动降置信是被否决的方案:会把「锚点错」和「结论是推测」两个信号坍缩成一个,
  且 root 外路径(仓库外文档引用)未必是幻觉。
- root 外锚点**零 fs 访问**:编排器直读文件绕过工具沙盒模型,只有
  canonicalize 后落在 `project_root` 前缀内的路径才读(绝对路径 join 时整体替换
  base、`..` 穿越由 canonicalize 展开,两者最终都落 `outside_root` 臂)。
  行数读取 16 MiB 字节帽(超帽 `unvalidated`)。

## 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `project_root` 缺失(root 不存在/经典路径) | 全部锚点 `unvalidated`,detail 照常落库 |
| 坏 JSON 列数据(MCP result / 转录 / GUI 卡) | 各消费方降级:`detail_warning` / 省略节 / 不叠记号,不炸 |
| end_discussion 无结构化参数 | detail NULL,全消费面文本兜底(旧场零回归) |
| serialize 失败(编排器) | warn + 丢弃 detail,finalize 照常 |
| 复用场重开 | `clear_group_chat_lifecycle` 三列全清——**漏清 detail = 续跑场消费上一场的断证结论**(专项断言锁定) |

## 5. Good/Base/Bad Cases

- Good:moderator 带锚点收官 → 落库 detail 带五臂 check → GUI 卡 ✓/⚠ 记号、
  MCP `result.detail`、双转录 `## conclusions` 节。
- Base:只发 `summary` 收官(旧剧本/朴素收官)→ 行为与本任务之前完全一致。
- Bad(设计上已排除):结构参数格式垃圾 → 整体降级为无结构化产物,收官不挂。

## 6. Tests Required

- 模块:`discussion_detail::tests` 解析四形态 + 校验五臂(tempdir fixture;
  `count_lines_capped` 的**尾换行语义**:以 `\n` 结尾的文件不加幽灵行)。
- E2E:`tests_group_chat::group_chat_structured_end_discussion_persists_validated_detail`
  (MockProvider 剧本带真实临时文件锚点 → 行 detail 落库 + check 各臂 + stance 不被改写)。
- 复用清空:`group_chat_second_run_clears_stale_stop_reason` 扩展(detail 不存活)。
- prompt:`tests_group_chat_prompts::moderator_prompt_teaches_structured_closing_contract`
  (基础教学三 stance + wrap-up 同款)。
- 前端:`DiscussionSummaryCard.test.ts` 7 用例(live input 渲染 / path+line 叠
  check / 坏 JSON / 无结构兜底)。
- JS:`group-chat-run.test.mjs`(渲染节各臂)+ `group-chat-mcp.test.mjs`
  (detail 透传 / 坏 JSON / 无键三臂)。

## 7. Wrong vs Correct

### Wrong

```rust
// 校验失败时改写 stance(被否决):机制层替消费方做认识论判断,
// 且 root 外引用(如 docs/ 路径笔误)被误降。
if anchor.check != Ok { conclusion.stance = Stance::Inferred; }
```

### Correct

```rust
// 只标注不修改:check 是事实,stance 是 moderator 的声明,两者并存;
// 「断证的 verified 条目」是有效状态,消费方自行解读。
a.check = Some(check_one(&a.path, a.line, &root_canon)); // claim/stance 原样
```

### Wrong(前端数据通道)

```ts
// 收官卡只从 session 行渲染 —— live 期(tool_result 刚到、行未 finalize)
// 结构化区整段空白,且行合并依赖轮询时序。
```

### Correct

```ts
// 双通道:live 期从 call.input(tool_use 参数,与消息同行、跨会话预览也正确)
// 渲染结构化区;收官后从 store 合并的行级 detail 按 path+line 叠 check 记号。
// readonly 跨会话预览不查 store —— currentSessionId 属于别的会话,叠上去就是
// 他场的校验结果。
```
