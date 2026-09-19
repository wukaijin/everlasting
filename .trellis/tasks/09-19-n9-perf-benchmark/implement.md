# Implement — N9 性能基准

> 评审修订版(session `448f6601` 结论已并入,见 design §10)。执行按 PR 切分;每步验证命令在该步内。
> 前置:design.md 定稿(待用户裁定 N2 bench 对象后冻结)。

## PR1 后端 bench 基建(B1 + B2)

1. **探针(首步,5 分钟)**:`cargo check -p everlasting --lib --features bench`;坐实 tempfile dev-dep 解析失败 → tempfile 转 optional + `bench = ["dep:tempfile"]`。
2. `app/src-tauri/Cargo.toml`:`[features] bench` + criterion 0.5 dev-dep + `[[bench]] harness/db` 两目标 `required-features = ["bench"]`。
3. **bench_api 再导出**(design §1.1 方案,非原 pub(crate) 门):`#[cfg(feature = "bench")] pub mod bench_api`,再导出 make_harness / chat_loop_deps / chat_loop_request / MockEmitter / db 迁移入口 / seed helper;`agent/tests_common.rs` 被导项提 pub(测试树引用零改动);bench 只准 use bench_api(依赖方向收口)。
4. profile 草稿 JSON(手写混合比,text/tool 对/thinking/长短参数化)入任务目录;`bench_support::seed_session(pool, profile, n)` 读它。
5. `benches/harness.rs`:h1/h2/h3/h3b/h4 × config 档(off/on);**iter_batched 每 iteration 测量外重建 session**(先于起始值校准);h3 显式 sample_size / measurement_time。
6. `benches/db_bench.rs`:b1 load_session(1k/10k)/ b2 persist_turn / b3 finalize_turn_persist(对象按用户裁定,见 design §10 开放问题)/ b5 删除后缀曲线;**双档分表**(内存档 + disk 档 init_pool + ext4 tempdir,禁 /mnt/*)。
7. 本机 `cargo bench -p everlasting` 出首批数字。

**验证**:
```bash
cargo check -p everlasting --lib --features bench   # 探针 + 门
cargo test -p everlasting --lib                     # 全量回归(测试树零改动)
cargo bench -p everlasting                          # 分布产出
cargo check -p everlasting                          # 默认构建零 feature 变化
```

## PR2 B3 双路径 + 画像固化 + 落档

1. **探针**:MockProvider 经 tmpdir config 被 chat 路由选中可行性;不行 → B3 = 路径 A + B1 差值分解,落档标注边界。
2. `benches/sse_bench.rs`:路径 A(replay TTFB)+ 路径 B(live 首事件),oneshot + build_router,criterion async。
3. 画像 SQL(sqlite3 -readonly):messages/session 分布、F5 列分布、tool duration 分布、内容形态混合比 → **只改 profile JSON 数字不改结构**,复跑校准种子。
4. `.trellis/spec/backend/perf-baseline.md` 落档(含归因式 B3−B1、统计语义、ext4 约束、更新纪律触发器)+ `spec/backend/index.md` 登记 + bench feature 勿滥用注记。

**验证**:
```bash
cargo bench -p everlasting --bench sse_bench
sqlite3 -readonly .../*.db "<画像 SQL>"              # 数字进 profile + 落档附录
```

## PR3 F1 前端 Playwright bench + CI 编译门

1. gen-fixtures.mjs:读 profile JSON 生成 100/1k/10k 档种子(与 B2 同源)。
2. `app/bench/*.bench.ts` + `playwright.bench.config.ts`:f1 mount(stick-to-bottom 收敛终点)/ f2 滚动+longtask / f3(可选)memory / f4 流式回放(10k,与 h3 成对);median+P75+max,10-15 run;报告落 `out/bench-fe/`(gitignore 确认)+ 人工检查列。
3. `app/package.json`:`"bench:fe": "playwright test -c playwright.bench.config.ts"`;e2e README 补边界一节。
4. **CI 编译门**(数字不进门禁,编译面进):CI Rust job 追加 `cargo check -p everlasting --lib --features bench --benches`;前端 bench 文件进 typecheck 面。

**验证**:
```bash
cd app && pnpm test && pnpm test:e2e                # 门禁零触碰
cd app && pnpm bench:fe                             # 三量级 + f4 报告产出
cargo check -p everlasting --lib --features bench --benches   # CI 门命令本地预演
```

## 收尾

- check 子代理全量:lint / type / vitest / cargo / CI 预演;perf-baseline.md 终稿含环境头与首批基线;journal 记录;归档流程照旧。

## 风险文件与回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `agent/tests_common.rs`(被导项提 pub) | 可见性放宽的涟漪 | bench_api 单一出口收口;PR1 独立提交可 revert |
| `app/src-tauri/Cargo.toml` + lockfile | criterion/tempfile 依赖变化 | dev/optional-only,revert 即缩 |
| `.github/workflows/*`(编译门) | CI 面 | 单命令追加,revert 即回 |
| `app/bench/` + `playwright.bench.config.ts` | 与 e2e 门禁耦合 | 独立目录独立 config,revert 单 PR |
| `spec/backend/index.md` | 登记遗漏 | 收尾 checklist 项 |

## 待办

- [x] 用户裁定:N2 bench 对象 = finalize_turn_persist(2026-09-19,design §10 已记录)
- [x] PRD convergence pass(2026-09-19)
- [x] 用户终审 ok(2026-09-19)→ `task.py start`
