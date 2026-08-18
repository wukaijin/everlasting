# Design — 手动 /compact 命令入口

> 决议依据:prd.md D1-D7。行号基线:main @ 08-18(探索报告锚点)。

## 1. 总览

```
前端触发(两条路,同一 handler):
  palette 选中 /compact ──┐
  直输 "/compact <focus>" ─┴─ ChatInput executeBuiltin("compact", focus)
                             └─ transport.invoke("compact_session", { sessionId, focus })
后端执行(空闲期,turn 边界外):
  gate(单聊)→ in-flight 拒绝 → provider 解析 → DB 行加载
  → 水位 anchor(最新 compaction_summary 行)→ 保留区计算 → 摘要 prompt(+focus/+prior)
  → provider.send 一次 → 成功落摘要行(MAX(seq)+1, trigger=manual)→ 响应载荷
下一次请求:init 的 apply_compaction_watermark 自然吃到新水位(不重付,零 init 改动)
```

## 2. 后端:compact_session 命令

### 2.1 注册面(参照 group_chat_cache_rates 全链路)

- 实现:`commands/sessions.rs` `compact_session_inner(&Arc<AppState>, session_id, focus)` + `#[tauri::command]` 薄壳;
- `lib.rs` `generate_handler!` 列表;
- daemon:`daemon/routes/sessions.rs` handler(Request struct `Deserialize`)+ router 路由 + oneshot 冒烟测试(backend/daemon-server.md §6);
- `app/src/transport/http.ts` `CMD_TO_DOMAIN` 加 `compact_session: "sessions"`。

### 2.2 编排流程(`agent/compaction.rs` 新 pub(crate) 入口,如 `run_manual_compaction`)

1. **scope gate**:load session 行,`session_type == GroupChat` → Err(口径同 C3+ gate 测试)。worker 无 session 行标记(判定来自 chat 请求参数),manual 入口默认主 loop 单聊——worker 上下文由其 C1 resume 兜底,不适用手动收窗(实现时复核 `init.rs:419-436` 的 `compaction_on` 判定来源,如 worker 可从 DB 判则同拒)。
2. **in-flight guard**(D4):`state.session_active_request` lock 含 session_id → Err "当前有轮次进行中,请先停止"。
3. **provider 解析**:`lookup_provider_for_session(&session_id, &db, &catalog)`(agent/chat.rs:650)→ `{provider, context_window, model_display_name}`——与 chat 主路径同源(session override → global default)。
4. **行加载**:`load_session` 的 `messages`(DB 行带 metadata;空闲期前端 reloadAfterFinalize 已重灌,wire↔DB 1:1 前提天然成立)。
5. **水位 anchor**:倒序找最新 `metadata.kind == "compaction_summary"` 行 → `SummaryAnchor { seq, content(纯摘要), cutoff }`;无 → None。提取 `apply_compaction_watermark` 第 1-2 步为独立小函数 `find_latest_summary_anchor(rows)`(配单测),两处共用。
6. **candidate 区** = `rows.filter(seq > anchor.cutoff 且 kind ≠ compaction_summary)`(无 anchor → 全部非 summary 行)。
7. **保留区**:复用 `compute_preservation_region` 语义,candidate 转 `ChatMessage` 后从尾向前累积(clamp(15k, 10%窗, 25k),组边界、typed-user 护栏);**synthetic 等价偏移 = 0**(DB 行不含合成头,无需排除)。空待压区(cut == 0,水位后历史全进保留区)→ Err "无可压缩内容"(AC1)。
8. **prompt**:`build_compaction_prompt` **加 `focus: Option<&str>` 参数**(auto 路径传 None,零行为变化):focus 存在时在模板头部追加定向指令块(如 `FOCUS INSTRUCTIONS FROM THE USER: {focus}`——实现时措辞对齐模板风格);prior anchor 走既有 `<prior-summary>` 块。
9. **LLM 调用**:同 auto 路径参数——无 tools、禁 thinking、输出上限 4k、retry 包裹;剥壳取 assistant text。
10. **落库**:`next_seq = MAX(seq)+1`(in-flight guard 保证无并发 persist,安全);`insert_compaction_summary`(吃传入 seq 插入,返回推进值)写 metadata:

```json
{ "kind": "compaction_summary", "cutoff_seq": N, "preserve_from_seq": M,
  "tokens_before": …, "tokens_after": …, "trigger": "manual",
  "focus": "聚焦 API 变更"(可null), "prior_summary_seq": 旧摘要行seq|null,
  "model": …, "summary_usage": {…} }
```

cutoff = candidate 压缩区末行 seq(精确值,**不用摘要行 seq-1 近似**——C3+ §4.3 修订教训);preserve_from = 保留区首行实际 seq。
11. **熔断**(D6):入口不查 `is_tripped`;成功 `record_success`(解熔断),失败 `record_failure`(共享信号,3 次连带影响 auto 路径——预期行为)。
12. **响应载荷** `ManualCompactionResult { cutoff_seq, tokens_before, tokens_after, summary_usage, model }`(serde snake_case,TS 镜像类型)。

### 2.3 失败语义(D5)

摘要失败 / insert 失败 → `record_failure` + Err 返回(前端 toast);**零 DB 写入**。机械丢组不在空闲期执行(无 in-loop context 可修,无持久化语义);下一次请求超线时 drive_turn 的既有降级链自然接管。

## 3. 前端

### 3.1 注册与面板

- `resource_loader.rs` `BUILTIN_COMMANDS` + `{ name: "compact", description: "压缩上下文(摘要收窗)" }`;`BuiltinCommand` 加 `argument_hint: Option<&'static str>` 字段,compact 给 `"[focus] 可选定向说明"`,panel.rs builtin 映射(`:283` 现 hardcode None)改读该字段(其余三命令 None,面板显示不变)。

### 3.2 handler 与直输分发(D2/D3)

- `ChatInput.vue` builtin switch 提取 `executeBuiltin(name, focus)` 共享函数;`case "compact"`:
  - focus 来源:palette 路径读 token 剥离后的编辑器残留文本(用户 `/compact 聚焦API` 后开面板选中 → 残留即 focus),直输路径为 rest-of-line;trim 后空 → None;执行后清空编辑器;
  - `transport.invoke("compact_session", { sessionId, focus })`;toast "正在压缩…" → 成功 toast `已压缩 {before}→{after} tokens` + 触发消息流 reload(load_session,同 /clear 后刷新路径,摘要行走既有 kind=compaction_summary 渲染);失败 toast error。
- **通用直输拦截**:新纯函数 `matchBuiltinCommandInput(text, names) -> Option<{ name, rest }>`(app/src/utils,首 token 需 `text.trim_start()` 后以 `/` 起、token 名精确匹配 builtin 名);ChatInput 提交路径在 emit send 之前拦截 → `executeBuiltin(name, rest)`;不匹配照常 emit(原行为)。palette 打开时 Enter 走面板选中(chatInputCodeMirror 现有处理),与拦截不冲突。`/help` `/clear` `/new` 直输顺带获得与面板一致的行为(D3)。
- streaming 中前端本就锁发送;后端 in-flight guard 兜底(双保险)。

## 4. 观测

- 摘要行 metadata `trigger:"manual"` + `focus`(D7);无 turn → 不写 `compaction_json`;usage 只进 metadata + 响应载荷(不混入 turn usage 口径)。
- `tracing::info!` 一条 manual compaction 结果(session_id / cutoff / before→after / model),对齐 chat 侧日志风格。

## 5. 边界情况

| 场景 | 处理 |
|------|------|
| 低 context(未超 0.85 线) | 允许(R1);但待压区空 → Err "无可压缩内容" |
| 已有水位 | prior 增量合并(§2.2-8);旧摘要行保留(死数据可审计,C3+ §2.2) |
| in-flight | 拒绝(D4) |
| 熔断 tripped | 手动照跑(D6) |
| 群聊 | 拒绝(§2.2-1) |
| 手动压缩后用户 D3 编辑历史 | cascade 删摘要行 → 水位自愈(C3+ §2.2 同款,零新代码) |
| insert 撞主键(理论竞态) | in-flight guard 下不发生;Err 路径兜底 |

## 6. 测试计划

**Rust 单测**(compaction.rs tests + 新增 manual 入口测试,MockProvider + seed_history 预落 DB 模式):
- `find_latest_summary_anchor`:命中/未命中;
- `build_compaction_prompt` focus 注入(auto 传 None 回归);
- manual 编排:低阈值成功落行(trigger=manual)/ focus 落 prompt / 水位增量(prior_summary_seq 指旧行)/ 失败零写入 + record_failure / in-flight 拒绝 / 空待压区 Err / 群聊拒绝 / tripped 绕过 + 成功解熔断 / seq = MAX+1;
- daemon route oneshot 冒烟。

**FE vitest**:`matchBuiltinCommandInput`(匹配/不匹配/前导空白/仅斜杠);compact case 的 focus 残留读取(可测则测)。

**mock 端到端**(AC7 前半):seed 长历史 → compact_session → 新请求 run_chat_loop → 断言首条 context 为摘要、context token 骤降、摘要调用不重复。

**live**:turn-smoke 扩展 `--compact` 步骤——跑完一轮(idle)后调 `POST /api/v1/sessions/compact_session`,断言摘要行落库(trigger=manual)、再跑一轮 context_input 显著下降、跑完自动删 session。

## 7. 回滚

- 命令与拦截均为纯增量;摘除 `compact` case 后其余三命令直输行为不变(通用拦截对它们只是"面板同款行为"的修正,若不想要可整体回滚拦截函数)。
- 摘要行是普通 messages 行,回滚代码后无害(C3+ §10 同款)。
