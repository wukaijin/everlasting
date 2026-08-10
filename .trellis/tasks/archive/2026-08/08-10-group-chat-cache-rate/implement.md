# Implement — 群聊参与者/主持人最近一次缓存率显示

## 执行清单

按依赖顺序,前端步骤依赖后端 IPC 契约(design.md)。

### 后端 (app/src-tauri)

1. **`db/trace.rs`**:新增 `SpeakerCacheUsage` struct + `list_speaker_cache_usage(pool, session_id)`,实现 design.md SQL(json_extract 取 cache_read / context_input;`t.token_usage_json IS NOT NULL` + max-seq 子查询)。
   - 参照现有 `list_turn_traces`(`db/trace.rs:168-200`)的写法和注释纪律。
2. **`db/trace.rs` 测试**(`db/trace_tests.rs` 或现有 trace 测试文件追加):构造 session + messages(多 speaker、多轮、role=user 干扰行、speaker=NULL 干扰行)+ turn_trace(token_usage_json 正常 / NULL / 缺 context_input),断言:
   - 每 speaker 只返回最近一次(max seq)轮次的数字;
   - 无 usage 轮次被跳过;
   - legacy 行(无 context_input)保留原样返回(前端负责过滤)。
3. **`commands/sessions.rs`**:`group_chat_cache_rates` 命令,inner + wrapper 模式(照抄 `commands/permissions.rs:397-411`),挂到 `lib.rs` invoke_handler(sessions 命令区块)。

### 前端 (app)

4. **`utils/tokenUsage.ts`**:追加 `cacheRatePercent(cacheRead, contextInput): number | null`(contextInput <= 0 → null;否则 Math.round 整数百分比)。补 `tokenUsage.test.ts` 用例(0 分母、正常、非整数舍入)。
5. **`stores/chat.types.ts`**(或弹窗内定义):`SpeakerCacheUsage` 接口。
6. **`components/chat/GroupChatConfigModal.vue`**:
   - edit 模式打开时 invoke `group_chat_cache_rates` → `cacheRates: Map<speaker, SpeakerCacheUsage>`(失败静默 + console.error);
   - 参与者行追加只读"缓存率 X% / —"行;
   - 弹窗底部新增只读"主持人"区(speaker 固定 `"moderator"`,model label 用现有 `modelLabel(currentSession.model_id)`,currentSession 从 props/sessionId 关联到 store);
   - create 模式不加载不显示。
7. **`GroupChatConfigModal.test.ts` 扩展**:edit 模式 mock invoke 返回 → 断言参与者行 + 主持人区渲染;无数据 → "—";create 模式不显示。

### 收尾

8. `pnpm test`(app)+ `cargo test --lib`(相关模块过滤)全绿。
9. `pnpm lint` + `cargo clippy`(如项目惯例)。

## 验证命令

```bash
# 后端(相关模块,避免全量 1657 个)
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib "trace" && cargo test --lib "sessions"

# 前端
cd app && pnpm test -- --run  # 或 pnpm test 交互
cd app && pnpm lint
```

## 风险文件 / 回滚点

- `db/trace.rs` — 纯新增函数 + 测试,不动既有函数。
- `commands/sessions.rs` + `lib.rs` invoke_handler — 纯新增命令;移除注册即可回滚。
- `GroupChatConfigModal.vue` — 弹窗改动,风险最高(既有 15+ 测试断言结构);保持既有行结构不变、只追加只读元素。
- 无 schema / 迁移 / 数据写入变更 → 回滚零成本。

## task.py start 前复查

- [ ] prd.md / design.md / implement.md 评审通过
- [ ] jsonl 上下文就绪(见下)

## jsonl 上下文(curate 用)

- implement.jsonl / check.jsonl 需真实条目,至少包含:
  - `.trellis/spec/backend/*` 相关 index(数据库 / 命令层规范)
  - `.trellis/spec/frontend/*` 相关 index(组件 / 测试规范)
  - 本任务 design.md / prd.md(子代理上下文)
