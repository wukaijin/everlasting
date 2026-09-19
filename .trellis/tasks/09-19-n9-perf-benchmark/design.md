# Design — N9 性能基准(criterion + Playwright 真浏览器)

> 评审修订版(2026-09-19,session `448f6601`,review preset 四视角,12 条结论全采纳,详见 §10)。
> 前置阅读:prd.md(需求与勘察事实)。本文档只讲怎么做与为什么。

## 0. 总架构

三个测量面,两套工具链,一份落档:

| 面 | 工具 | 形态 | 产出 |
|---|---|---|---|
| B1 harness 开销(run_chat_loop 整轮) | cargo criterion | `app/src-tauri/benches/` 独立 bench crate | P50/P99 分布 + baseline 对比 |
| B2 DB 读写(load_session / persist / 删除后缀) | cargo criterion | 同上(共享 seed helper) | 同上 |
| B3 SSE 双路径(replay TTFB / live 首事件) | cargo criterion(async) | 同上 | 同上 + 归因式 |
| F1 前端渲染(mount/滚动/流式回放) | Playwright 真浏览器 | `app/bench/` 独立目录 + 独立 config | JSON 报告(median/P75/max) |
| 画像(辅证) | sqlite3 -readonly | 一次性 SQL → **入库 profile JSON** | 真实负载分布 → 种子单一出处 |

统一落档 `.trellis/spec/backend/perf-baseline.md`(口径 + 基线 + 复跑命令)。

**核心口径(全任务不变量)**:所有数字只做**同机自比较**;WSL2 抖动下绝对值跨机无意义;每份报告带环境头(CPU 型号 / nproc / 电源 / 负载来源)。**统计语义**:P50/P99 列只做指示值,回归判定一律用 criterion change detection(斜率 + 置信区间),不用裸阈值比较。

## 1. 后端基建:`bench` feature 门 + criterion

### 1.1 可见性(评审结论 #1/#2/#3 修订:原 pub(crate) 门方案编译不过)

编译事实(benches = 独立 crate):`pub(crate)` 对 benches 不可见;`lib.rs` 的 `mod agent; mod db;` 私有声明断掉祖先链——cfg(test) 树内的构造件无论怎样提可见性都到不了 benches。构造件实际位置 = `agent/tests_common.rs`(模块级门 = 文件第 7 行一行 `#![cfg(test)]`,非深挂树,勘误原「tests_agent_loop/tests_common」表述)。

**方案(唯一可达路径)**:

```toml
# app/src-tauri/Cargo.toml
[features]
bench = ["dep:tempfile"]   # 若 #2 探针坐实 tempfile 必须随 feature(见下)
[dev-dependencies]
criterion = "0.5"
[[bench]]
name = "harness"
harness = false
required-features = ["bench"]
```

```rust
// lib.rs(或 agent/db 各自):
#[cfg(feature = "bench")]
pub mod bench_api;   // 模块内 pub use 再导出 benches 需要的一切:
                     // run_chat_loop 构造件(make_harness/chat_loop_deps/
                     // chat_loop_request/MockEmitter)、db 迁移入口、seed helper
```

被再导出的项在 `tests_common.rs`(cfg(test) 文件)内从 `pub(crate)` 提为 `pub`;**测试树引用零改动**(pub 是 pub(crate) 的超集)。

**探针(PR1 首步,5 分钟)**:`cargo check -p everlasting --lib --features bench`。风险:`TestHarness` 持 `tempfile::TempDir`(tests_common.rs:223)而 tempfile 是 dev-dependency——**dev-deps 不参与 bench 目标的 lib 构建**,`--features bench` 可能解析失败。坐实则 tempfile 转 `[dependencies] optional = true` + `bench = ["dep:tempfile"]`。

**依赖方向收口(防腐烂,评审结论 #12)**:benches 只准 `use everlasting::bench_api::*` / 既有 pub 类型,禁止绕过 bench_api 直捅 crate 内部;PR 检查项写进 spec。

### 1.2 权衡(评审结论 #3 维持)

criterion vs crate 内自写计时:后者零可见性改动,但无 baseline 对比与统计,而「回归可见性」恰是本任务核心价值——criterion 的 change detection(斜率+置信区间)正中靶心。选 criterion,代价 = dev-only lockfile 增量 + feature 门改动。

## 2. B1 harness 开销 bench

- **对象**:`run_chat_loop` 完整一轮(MockProvider 脚本驱动,MockEmitter 收事件,真实 pool 落库)——覆盖 load_session 回放、context 组装、system prompt、memory 加载、tools[] schema 序列化、provider 调度、tool 执行、persist、事件 emit 全链。网络变量已被 MockProvider 摘除。
- **场景矩阵**(评审结论 #4/#6 修订):

| 组 | session 预置 | 脚本 | config 档 | 度量什么 |
|---|---|---|---|---|
| h1 | 空 session(1 条 user) | 纯文本一轮 | off/on | 冷启动开销 |
| h2 | 预种 1k 消息 | 纯文本一轮 | off/on | 历史回放成本随 N 增长 |
| h3 | 预种 10k 消息 | 纯文本一轮 | off/on | 长会话启动(N4/N2 消费;**口径注**:启动即自调 load_session,h3 = DB 读+rehydrate+context 组装三项混叠,单函数分解靠 h3b) |
| h3b | 与 h3 同种子 | 单函数组:历史组装 / rehydrate 各自单 bench | — | 从「可选扩展」升 MVP 必做:把 h3 混叠项拆开可归因 |
| h4 | 空 session | tool_use→tool_result→文本(2 轮) | off | 工具回路开销 |

- **config 档列**:`llm_compaction_enabled` off/on 两档——`make_harness` 继承测试档 off 而生产为 on,只量 off 档会把压缩分支的成本漏掉(评审结论 #6)。
- **迭代隔离(评审结论 #4,硬约束)**:h2/h3 每次真落库,iteration 间累积追加行且会撞 `UNIQUE(session_id, seq)`(messages.rs:55)——**必须 `iter_batched`/`PerIteration` 每 iteration 在测量外 setup 重建 session**;此约束置于 implement.md 起始值校准步骤之前。
- MockProvider 脚本事件序列与 tests_agent_loop 既有用法同构(Start/Delta/Done),不发明协议形状。
- h3 单次 iteration 秒级:显式 `sample_size` / `measurement_time` 配置(评审结论 #11)。

## 3. B2 DB 读写 bench

- **双档(评审结论 #5 修订,原「内存库=下界」口径对 N2 测错维度)**:
  - **内存档**(`test_pool` 形态,sqlite::memory,无 WAL):量查询/行映射 CPU 成本。
  - **disk 档**(`init_pool`(db/migrations/pool.rs:33,已 pub 免门)+ tempdir `.db`,**tempdir 落 ext4(/tmp),严禁 /mnt/***)**:量含 WAL/fsync 的真实写成本。**两档分表,明文禁止内存档用于 N2 推算**(内存档抹掉 auto-commit fsync 成本);N2 checkpoint 成本模型 = **disk 档数字 × 每轮提交次数**。
- **量三个方向**:
  - `load_session`(查询 + 行映射)P50/P99——session 打开路径。
  - `persist_turn` / `finalize_turn_persist` 单条写入(disk 档为主)——N2 auto-commit 成本模型。
  - **b5(评审结论 #7 新增)**:按 seq 删除后缀(N 行 DELETE + 单次提交)的 P50 随表增长曲线——N2 revert = reset 语义的删除成本,与 INSERT 不同量纲,不量则 N2 成本模型只有一半。
- **种子(评审结论 #10 双侧单一出处)**:`bench_support::seed_session(pool, profile, n)`;profile = 入库 JSON(§6 画像固化),PR1 落 profile 草稿(手写混合比),PR2 画像后**只改数字不改结构**。消息形态混合(纯 text / tool_use+tool_result 对 / thinking / 长短参数化)。

## 4. B3 SSE 双路径(评审结论 #8 重定义;降级线降格脚注)

原「SSE 首字节」在现协议下**无定义**:首连的 replay 为空,字面「第一条 event 字节」会量到 30s KeepAlive 心跳。重定义:

- **路径 A(replay TTFB)**:已有多事件的 session 首连,量请求到达 → replay 首字节。
- **路径 B(live 首事件)**:chat 请求发出 → 第一条 live event 上网线。
- **归因式(落档必写)**:`B3(路径B) − B1(h1) = daemon HTTP 面净开销`(路由 + 会话解析 + SSE registry + 序列化)。
- 形态:axum `tower::ServiceExt::oneshot` 直打 `build_router`(`daemon/server.rs:248`,`load_daemon_state` 免门先例成立——原 §8「AppState 构造重」风险行事实性不成立,降格为脚注),MockProvider 脚本控制,criterion async 计时。
- **实施期探针(PR2 前置)**:MockProvider 能否经 tmpdir config 被 chat 路由选中(route 测试现只用模型名且只断言受理);不行则 B3 = 路径 A + B1 差值分解,落档如实标注测量边界。
- 排除:真实网络栈(TCP/WebKit)延迟——F5 生产观测与真浏览器体验的事,不受控不进 bench。

## 5. F1 前端渲染基准(Playwright)

- **隔离**(AC 硬约束):`app/bench/` 新目录 + `playwright.bench.config.ts` + `pnpm bench:fe`;CI 的 `pnpm test:e2e` 与 Playwright 主 config 均不触碰。**依赖方向收口:F1 只准依赖 `e2e/fixtures.ts` 的 world fixture**(评审结论 #12)。
- **测量项(评审结论 #9 重构为四组)**:
  - f1 mount:boot() 完成 → **MessageList 自身 stick-to-bottom 收敛循环退出**(MessageList.vue:273;双 rAF 系统性偏短——收敛循环未跑完就取样)。
  - f2 滚动:programmatic scroll 至底 ×3,帧间隔分布(rAF 时间戳)+ longtask 总时长/条数。
  - f3(可选)`performance.memory.usedJSHeapSize`,标注 blink 专有非标准。
  - f4 **流式回放组(新增)**:10k 种子 + fake EventSource 流式推事件到收敛——与 h3 同源(同一 profile/量级),构成前后端对子,正是 N4/N2 关心的「长会话里继续对话」场景。
- **报告**:JSON 落 `out/bench-fe/<ts>/`;**统计 = median + P75 + max,10-15 run**(评审结论 #11:原「5 run 取 P99」名不实,P99=max 却不叫 max);**人工检查列**(滚动 thumb 稳定性 / 锚定漂移——帧分布量不出的两项,评审结论 #9)。
- **种子单一出处**:gen-fixtures.mjs 与 B2 `seed_session` **同读一份 profile JSON**(评审结论 #10);从生产 session 导出(脱敏)参数化 100/1k/10k 档,防种子形状与真实 daemon 响应漂移。
- N4 消费形态:同脚本同种子复跑,虚拟化前后各量级一列对一列。

## 6. 真实负载画像(辅证,一次性 → 入库 profile)

`sqlite3 -readonly ~/.local/share/dev.everlasting.app/everlasting.db`:
- messages per session 分布(P50/P95/最大)——种子量级档选型依据;
- F5 列分布(ttfb/gen/total P50/P95)、`tool_result.duration_ms` 分布、消息内容形态混合比(text/tool 对/thinking 占比)。
- **产出固化为入库 profile JSON**(任务目录内,进 git):B2 `seed_session` 与 F1 gen-fixtures 同读;后续更新只改数字不改结构。

## 7. 落档:`.trellis/spec/backend/perf-baseline.md`

章节:①口径与环境记录法(CPU/电源/后台负载自查、跑法命令、防抖要点、同机自比较不变量、**统计语义**:P99 指示值 + change detection 判定、**tempdir 落 ext4 禁 /mnt/***)；②基线表(B1-B3 + F1 各组数字,内存/disk 分表);③**归因式**(B3−B1 = daemon HTTP 净开销);④复跑与对比法(criterion `--save-baseline`;Playwright 同参复跑);⑤画像附录(profile JSON 指针);⑥更新纪律(**触发器挂 spec 索引**:动 context 组装 / DB schema / render 路线的任务在 check 阶段复跑对应面并更新——评审结论 #12)。

## 8. 风险清单(评审修订)

| 风险 | 缓解 |
|---|---|
| criterion 传递依赖膨胀 lockfile | dev-only(+ tempfile 可能转 optional);记录进 spec;无运行时面 |
| bench 代码无人跑而腐烂(编译漂移) | **编译门进 CI**:`cargo check -p everlasting --lib --features bench --benches` + bench 文件进 typecheck——数字不进门禁(用户裁定不变),编译面进门禁(评审结论 #12) |
| 迭代累积污染 / UNIQUE 撞 | iter_batched 每 iteration 测量外重建 session(§2 硬约束) |
| 种子形状不代表真实负载 | 画像 profile 固化入库,B2/F1 双侧单一出处(§3/§5/§6) |
| WSL2 抖动 + /mnt 跨文件系统失真 | 同机自比较口径 + 环境头 + change detection;tempdir 只落 ext4 |
| bench 与 CI 门禁数字耦合 | 隔离双保险(目录 + config);CI 只收编译门 |
| ~~AppState 构造重导致 SSE 面难做~~ | 事实性不成立(build_router/load_daemon_state 免门先例),降格脚注;真风险 = MockProvider config 可选性(PR2 探针) |

## 9. 历史评审关注点(已全部有结论,留档)

原文七问(可见性方案 / criterion 取舍 / SSE 构造 / 矩阵充分性 / 种子代表性 / 统计口径 / Playwright 充分性)由 session `448f6601` 逐一闭环,结论与裁决见 §10。

## 10. 评审结论记录(session `448f6601`,2026-09-19,四视角 12 条全采纳)

| # | 结论(锚点核码状态) | 落点 |
|---|---|---|
| 1 | 原 §1.1 pub(crate) 门编译不过(benches 独立 crate + lib.rs 私有 mod 断祖先链);正解 = `#[cfg(feature="bench")] pub mod bench_api` 再导出,被导项提 pub,测试树零改动(verified) | §1.1 重写 |
| 2 | tempfile dev-dep 不进 bench 目标 lib 构建;PR1 首步探针,坐实则 optional + `bench=["dep:tempfile"]`(inferred) | §1.1 + implement PR1 |
| 3 | criterion 取舍成立;构造件实际在 `agent/tests_common.rs`(门一行),原路径表述勘误(verified) | §1.1/1.2 |
| 4 | h2/h3 迭代累积污染 + `UNIQUE(session_id,seq)` 撞;iter_batched 每 iteration 重建 session(verified) | §2 |
| 5 | 内存库对 N2 测错维度(无 WAL 抹 fsync);B2 双档分表,disk 档必加,N2 推算明文只认 disk 档(verified) | §3 |
| 6 | run_chat_loop 启动即 load_session,h3 三项混叠;h3b 升 MVP;矩阵加 llm_compaction_enabled off/on 列(verified) | §2 |
| 7 | 缺 b5 反向写组(按 seq 删除后缀曲线);否则 N2 revert 成本模型只有一半(verified) | §3 |
| 8 | B3「首字节」无定义(空 replay + 30s KeepAlive);重定义路径 A/B + 归因式 B3−B1;降级线降格脚注(verified) | §4 |
| 9 | F1 四组重构:mount 终点 = stick-to-bottom 收敛;新增 f4 流式回放与 h3 成对;报告加人工检查列(verified) | §5 |
| 10 | 种子单一出处双侧扩展:profile 固化入库 JSON,B2/F1 同读;PR1 草稿 → PR2 只改数字(verified) | §3/§5/§6 |
| 11 | 统计三修:P99 只做指示值、判定走 change detection;h3 显式 sample_size/measurement_time;F1 改 median+max 10-15 run;tempdir 落 ext4 禁 /mnt/*(inferred) | §0/§2/§5/§7 |
| 12 | 防腐烂:编译门进 CI(数字门禁裁定不变,编译面非抖动);依赖方向收口(bench 只 use bench_api;F1 只依赖 fixtures world);更新纪律触发器挂 spec 索引(inferred) | §1.1/§7/§8 + implement PR3 |

**开放问题(已全部闭环)**:
- **已裁定(2026-09-19 用户,采纳推荐)**:N2 checkpoint bench 对象 = `finalize_turn_persist`(db/sessions/messages.rs:209)——N2 落地路径 = turn 边界 auto-commit,写路径即每 turn persist/finalize;群聊 UPSERT 语义无关。b 组 = persist_turn(b2)/ finalize_turn_persist(b3)/ 删除后缀(b5)。
- 实施期探针(不需裁定):tempfile 探针(PR1 首步);MockProvider 经 tmpdir config 被 chat 路由选中可行性(B3 路径 B 前置)。
