# 执行计划 — memory-block-governance

依据:PRD WP1-WP3 + design.md §3 + `research/code-map-20260815.md`(行号为 2026-08-15 基线,**实现前重扫**)。

## PR1 · WP1 度量(小而稳,先拿基线)

### Step 1.1 DB 层

1. `db/migrations/schema.rs`:建表语句 `turn_trace` 加 `memory_token INTEGER`(紧邻 `tools_token`,基线 :959)+ 幂等 backfill `add_turn_trace_column_if_missing(pool, "memory_token", "INTEGER")`(基线 :981 旁)。
2. `db/trace.rs`:`TurnTraceRow` 加 `memory_token: Option<i64>`(:48 旁);`upsert_turn_trace_token` 扩参 `memory_token: Option<u32>`(SQL 列清单 + excluded 同步);list 查询(:281/:300)带列。
3. grep 自查:`grep -n "tools_token" app/src-tauri/src/db/`——每个出现点的紧邻处补 memory_token,防漏。

### Step 1.2 agent loop 传递

1. `chat_loop/init.rs`:`LoopInit` 加 `memory_token: Option<u32>`;注入处(:378-397)`build_instructions_blocks` 产出后,对全部 blocks 的 text 拼接跑 `crate::memory::tokens::count_tokens`(非空才计)。
2. `chat_loop/drive.rs`:Done 事件 upsert 点(:860)`upsert_turn_trace_token` 调用补传 `loop_init.memory_token`(或经既有 LoopInit 传递链,重扫确认字段可达性)。
3. 注意口径注释(同 tools_token :604):估算的是**注入内容**(banner + wrappers + body),与 banner 显示的"每文件 tokens 和"有少量 wrapper 差异,注释声明。
4. ⚠️ **只量 init.rs 路径**(评审更正):worker 的 memory 注入在 `agent/subagent/prompt.rs:63` `build_worker_messages`(独立构造,prompt.rs **一行不动**),worker 的 turn_trace 行 memory_token 留 null(design §3.5a)。

### Step 1.3 前端 + 烟测

1. `src/types/turnTrace.ts`:`toolsToken`(:123)旁加 `memoryToken?: number | null`(镜像注释同款)。
2. `src/components/trace/TurnCard.vue`:复刻 `toolsPct`(:159-170)为 `memoryPct` computed;模板 tools cell 旁加 memory cell(同布局;null 时隐藏)。trace.test.ts 补断言。
3. `scripts/turn-smoke.sh`:turn_trace 查询加 memory_token,输出行加 `memory=<n> (<pct>%)`;**扩 `--turns N`**(默认 1;现脚本单轮单 POST,:89——AC4 的第二轮 cache 率需同一 session 连发两条消息,轮询输出逐 turn 的 memory/cache_read)。

### Step 1.4 验证

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app && pnpm test && pnpm build
./scripts/turn-smoke.sh    # 重编 release + 重启 :7456 daemon 后;结果记 research/baseline.md
```

## PR2 · WP2 分级注入 digest + load_memory_sections

### Step 2.0 重扫

```bash
grep -rn "build_instructions_blocks" app/src-tauri/src --include="*.rs"  # 调用侧:预期 init.rs + subagent/prompt.rs + memory/tests.rs(prompt.rs 不动,见 design §3.5a)
grep -rn "load_tool_schemas" app/src-tauri/src/agent/chat_loop/tools.rs   # 执行拦截点(serial 顶部按名拦截,基线 :792-828)
grep -n "stub_loaded.clear" app/src-tauri/src/commands/sessions.rs        # registry 清理挂点(基线 :363,digest registry 同款补一行)
```

### Step 2.1 digest 模块(纯函数,先行可测)

1. 新 `memory/digest.rs`:fence-aware 切节状态机(`^```` 翻转 in_fence;`!in_fence && ^#{1,3} ` 为节界;Preamble 单列)+ 目录生成(标题 + ≤120 chars 首句,**纯 fence 节回退取节内首个非空行**,design §3.2;目录头一行 `[<file> · digest: N sections, ~X tokens full …]`,尾一行调用指引,调用指引里的层名用 banner label 命名空间)。
2. 单测重点:fenced 内 `# 注释` 不切节(repo CLAUDE.md 实测快照断言:7 节)、无 header 文件、空文件、`#`/`##`/`###` 混级、fence 未闭合容错、Common Commands 整节单 code block 的回退路径。
3. 层 tokens ≤600 → 不 digest 直接 Full(design §3.1)。

### Step 2.2 注入侧改造(只动 init.rs 路径)

1. `build_instructions_blocks` 扩参(digest 开关 + registry loaded-set + per-layer tier 判定):Digest 层 body = 目录(已 loaded 节的全文**追加在目录后**);Full 层不变;块序/banner/包裹语义不动(不变量 I1/I3)。**`subagent/prompt.rs:63` 的 worker 调用点不传新参数、一行不动**(design §3.5a;扩参用 wrapper 或默认参数保该调用点编译不变,重扫后定形)。
2. `chat_loop/init.rs` 调用点接 gate:`memory_digest_enabled`(chat_loop.rs 每 request 读一次,:613-618 模式)&& `!effective_is_worker` && `!is_group_chat`;WP1 的 memory_token 计算自动反映 digest 后体积(无需改)。
3. 群聊/worker 路径零改动验证:gate off 分支注入内容与现状逐字节一致(单测断言合成消息文本,两条路径各一)。

### Step 2.3 load_memory_sections 元工具 + registry

1. `MemoryDigestRegistry`(同 `StubRegistry`:`session_id → HashSet<section_key>`,key = `<banner label>#<节标题>`):存储挂 `AppState`(state.rs :208 `stub_loaded` 旁);**清理点 `commands/sessions.rs:363`** delete_session 处同款补一行;**mtime 变更后陈旧 key 静默丢弃**(design §3.4)。
2. def 生成函数放 memory 模块(对应 `stub.rs:105` `load_tool_schemas_def` 先例):参数 `sections: string[]`,寻址 = banner label 命名空间 + `#节标题`,匹配 = 精确 → 唯一前缀/子串回退,`["all"]` 全量(design §3.3)。
3. **执行路由 = `chat_loop/tools.rs` serial 顶部按名拦截**(基线 :792-828 `load_tool_schemas` 分支旁同款):从 MemoryCache 取层内容 → 定位节 → 原文文本返回;未知节报错附层/节清单;read-only silent-allow。
4. drive.rs append 点同 `load_tool_schemas`(:583-590),gate 同源;注意 tools_token 会因此 +ε(元工具 def ~100-200 tok),AC2 净收益按 memory 降幅计。

### Step 2.4 验证(AC2-AC5)

```bash
# 单测 + 全量
cd app/src-tauri && PKG_CONFIG_PATH="…" cargo test --lib && cargo clippy -- -D warnings
cd app && pnpm test && pnpm build
# live:重编 release + 重启 daemon(pid kill,勿 pkill -f 端口串)
./scripts/turn-smoke.sh                 # AC2:首轮 memory_token ≤2500
./scripts/turn-smoke.sh --turns 2 --keep # AC4:同 session 双轮,第二轮 cache_read 率对比 digest-off 基线(评审更正:--keep 不发第二轮,必须用 --turns 2)
```

- AC3 真机:GUI 实跑"改一处前端样式 + 跑一轮测试"任务,观察模型是否按需拉节并遵循(TracePanel 看 load_memory_sections 调用)。
- AC5:开关 off + worker/群聊 session 注入断言单测。

## PR2 收尾 · WP3 沉淀

1. `.trellis/spec/backend/memory.md`:Scenario 1 加 digest/tier Decision 注记(2026-08-15):tier 规则、I1-I3 不变量、fence 切节契约、粘性 registry。
2. `.trellis/spec/backend/token-usage-tracking.md`:memory_token 口径段(与 tools_token 同款 no-double-count)。
3. `docs/BACKLOG.md` §3.1 进展更新 + `docs/ROADMAP.md` §1.2 加行(数据:首轮 memory 降幅 + 占比)。

## 回滚点

- PR1 独立可 revert(新列无害,旧行为 null)。
- PR2 开关 `memory_digest_enabled=off` 即回现状;极端 revert PR2 不伤 PR1。

## 风险提示(实现时盯)

- **双注入路径**(评审更正):`build_instructions_blocks` 有两个调用点(init.rs 主路径 + `subagent/prompt.rs:63` worker 路径)。扩参时保 prompt.rs 调用点不变;改完 grep 断言新逻辑只出现在 init.rs 侧。
- fence 状态机是本任务最高 bug 密度点——先写测试再写实现(repo CLAUDE.md 快照);Common Commands 整节单 code block 走回退路径。
- `LoopInit` 字段传递链较长(init → drive),编译期盯 dead_code 警告防"算了没落库"。
- turn-smoke 前置:二进制要含 PR1(memory_token 列缺失时脚本应像 tools_token 一样明确报错,补同款检测);`--turns` 是 PR1 交付物的一部分(AC4 依赖)。
- 群聊路径每次新建 StubRegistry(group_chat_loop.rs:350/558)是既有分叉——digest registry 只挂 AppState,别试图统一两条 registry 生命周期(v1 群聊豁免)。
