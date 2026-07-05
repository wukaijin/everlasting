# Design — CI 测试自动化管线

> 对应 `prd.md`。本文档记录 workflow 结构、系统依赖清单、cache 策略、fmt 处置与失败模式。实施细节走 `implement.md`。

## 1. 架构与边界

```
.github/workflows/ci.yml
├── on: push(main) + pull_request + workflow_dispatch
├── path filter: ignore **/*.md, docs/**, .trellis/**
└── jobs (并行)
    ├── rust (ubuntu-latest)
    │   ├── apt install 系统依赖
    │   ├── dtolnay/rust-toolchain@stable (含 rustfmt)
    │   ├── Swatinem/rust-cache@v2
    │   ├── cargo fmt --check   ← R4 gate
    │   └── cargo test --lib     ← 1274 单测
    └── frontend (ubuntu-latest)
        ├── pnpm/action-setup@v4
        ├── actions/setup-node@v4 (cache: pnpm, cache-dependency-path: app/pnpm-lock.yaml)
        ├── pnpm install --frozen-lockfile  (cwd: app)
        ├── pnpm test                      ← vitest run, 718 单测
        └── pnpm build                     ← vue-tsc --noEmit + vite build
```

**边界**：本 task 只动两个东西——新增 `.github/workflows/ci.yml` + `.github/workflows/` 下零依赖；以及一次全量 `cargo fmt`（机械改动，不动逻辑）。agent core / 前端业务代码零修改。

## 2. 系统依赖清单（rust job，apt）

Tauri 2 在 ubuntu runner 上的精简清单（已为 rustls 配置剔除 libssl-dev）：

```yaml
- name: Install Tauri system deps
  run: |
    sudo apt-get update
    sudo apt-get install -y \
      libwebkit2gtk-4.1-dev \
      build-essential \
      curl \
      wget \
      file \
      libxdo-dev \
      librsvg2-dev \
      libayatana-appindicator3-dev
```

**说明**：
- `libwebkit2gtk-4.1-dev` 是 Tauri 2 的硬依赖（Tauri 1 用 4.0，本项目 Tauri 2 必须 4.1）。**漏装这一条 `cargo test --lib` 编译期就 fail**（pkg-config 找不到 webkit2gtk-4.1）。
- `libayatana-appindicator3-dev` 是系统 tray 用，本项目当前没启 tray，但 Tauri 2 默认 feature 可能引用 → 保险起见装上（apt install 无害）。
- `libssl-dev` **故意省略**：`reqwest` 用 `rustls` feature（`Cargo.toml` 已确认 `default-features = false` + `rustls`），不链接 openssl-sys。如果未来切回 native-tls 再加。
- `pkg-config` ubuntu-latest runner 自带，不显式列。

## 3. Cache 策略

### Rust（rust job）
- `Swatinem/rust-cache@v2` —— 缓存 `~/.cargo/registry` + `target/`，key 含 `Cargo.lock` hash。
- **关键收益**：`git2` 的 `vendored-libgit2` 首次编译 30-90s 编 C 源；cache 命中后这部分产物复用，省 ~60s/次。
- workdir：`app/src-tauri`（rust-cache 的 `workspaces` 参数指向它）。

### Frontend（frontend job）
- `actions/setup-node@v4` 的 `cache: 'pnpm'` + `cache-dependency-path: 'app/pnpm-lock.yaml'`。
- pnpm 自身的 store cache 由 setup-node 接管，无需额外 action。

## 4. cargo fmt 处置（R4）

**现状**：`cargo fmt --check` 不干净（`at_file.rs:71/373` 等多处 diff，见 prd.md background）。

**步骤**：
1. 本地 `cd app/src-tauri && cargo fmt`（全量格式化，机械改动）。
2. **单独 commit**，message：`style(rust): cargo fmt 全量格式化 (CI fmt gate 前置)`。
3. workflow CI 文件**另一个 commit**：`ci: add GitHub Actions workflow (cargo test + fmt + vitest + vue-tsc)`。
4. 两 commit 同一 PR。

**Trade-off**：fmt diff 会很大（触及多个文件），review 噪音。但纯机械改动（rustfmt 不改语义），review 时可 `git diff --stat` 一眼扫过 + 信任工具。**不接受**任何手改夹带其中——如有手改需求，拆独立 commit。

**为什么不顺手清 clippy**：clippy 修复常含判断（改 `.clone()` → 引用、改 `unwrap()` → `?`），有逻辑风险，违反"纯 CI 引入"精神；且首次跑状态未知，可能爆几十个 warning 阻塞 PR。留独立 follow-up。

## 5. Path filter

```yaml
on:
  push:
    branches: [main]
    paths-ignore:
      - '**/*.md'
      - 'docs/**'
      - '.trellis/**'
  pull_request:
    paths-ignore:
      - '**/*.md'
      - 'docs/**'
      - '.trellis/**'
  workflow_dispatch:
```

**理由**：journal / spec / ROADMAP 改动频繁（每次 task 收尾都改 `.trellis/`），与代码无关，触发 CI 是浪费 GH Actions 免费分钟数（私有仓库 2000 min/mo，公开无限）。

**注意**：`.github/workflows/**` **不**进 paths-ignore——改 workflow 自身要触发验证。

## 6. 兼容性与迁移

- **零迁移**：纯加文件 + 机械 fmt，无 schema/wire/IPC 变更。
- **本地开发零影响**：CI 配置不反馈到本地工具链；本地仍走 `PKG_CONFIG_PATH` hack（HACKING-wsl 坑 1）。
- **fmt 改动可能与其他在途分支冲突**：若 main 上有未合并的 Rust PR，fmt 全量改动会让他们 rebase 时遇到格式冲突（机械，`cargo fmt` 一把即可解决）。当前无在途 Rust PR（git status clean，无 active task），风险低。

## 7. 失败模式与 rollback

| 失败场景 | 处置 |
|---|---|
| `cargo test --lib` 编译期缺 webkit | apt 清单加包（design §2 已覆盖） |
| `cargo fmt --check` 失败 | 本地 `cargo fmt` 后重推（首次已清，后续只在有人手改格式时触发） |
| rust-cache miss 导致 CI 慢 | 接受（首次必慢）；检查 `Cargo.lock` 是否频繁变动（key 不稳） |
| vitest jsdom 内存爆 | runner 4G 通常够；若爆，`NODE_OPTIONS=--max-old-space-size=4096` |
| 整个 workflow 误触发（docs PR 也跑） | path filter 调整 |

**Rollback 单元**：
- 整个 task = 一个 PR。回滚 = `git revert <merge-commit>`，恢复到无 CI 状态。
- fmt commit 与 workflow commit 分离 → 可单独 revert fmt（保留 CI），但会重新让 fmt gate 红，不推荐单独 revert。

## 8. Trade-off 总览

| 决策 | 选择 | 否决项 | 理由 |
|---|---|---|---|
| scope | A（test+type） | A+B（连 build 出包） | 防回归核心；出包留 release.yml |
| lint | fmt gate | fmt+clippy | clippy 首次状态未知 + 修复含判断，留 follow-up |
| job 结构 | 双 job 并行 | 单 job 串行 | frontend 不需 webkit，并行省时 |
| matrix | 单 ubuntu + stable | 多 OS/多 Rust | WSL-first，无 MSRV |
| 触发 | push+PR+dispatch | 仅 PR | 直推 main 也要防（虽不推荐直推） |

## 9. Follow-up（不进本 task，记账）

- `cargo clippy --deny warnings` gate（独立 task：先本地清 warning 再加 gate）。
- `release.yml`（tag-triggered，`pnpm tauri build` 出 .deb/.AppImage 上 GitHub Releases）。
- README badge 链接里的 `<owner>` 占位——本 task 提交时如已知 owner 填实，未知留占位 + 注释。

## 10. CI 暴露的预存 flaky（本 task 内修复）

CI 首次跑暴露两个预存 flaky（项目此前无 CI，本地跑 N 次侥幸过）。修复让 CI 信号可靠——否则 CI 偶发红，失去防回归价值。

### 10.1 background shell drain race（生产代码修复）

- **症状**：`agent_loop_drains_background_shell_notification_into_turn_2` CI 上 1/1274 失败（本地 1274/1274 全绿）。
- **根因**：`drain_notifications` 是 destructive pop，与 shell 完成 push 竞速。`echo`（fork+exec+exit+push 几 ms）可能晚于 turn 切换（μs 级），drain pop 空队列，notification 延迟到下 turn 或丢失。**这是真实生产 race**（不只是测试 flaky）：快 shell + loop 早结束 → notification 永不 drain，LLM 不知道 shell 完成。
- **修复**：`background_shell/in_memory.rs::drain_notifications` 加 race fix——队列空 + 有近期（< 200ms）running shell 时 yield+poll（5ms 间隔，cap 100ms），让 spawned task 完成 push。dev server（> 200ms old）不受影响；队列非空立即返回（原行为保留）；无 running shell 立即返回空（原行为保留）。
- **生产影响**：仅"队列空 + 刚 spawn shell running"边缘 case 加 ≤100ms wait；常见 case（队列非空 / 无 running / dev server）零开销。
- **验证**：10/10 单跑稳定 + 全量回归无新失败。

### 10.2 loader mtime fence 精度 flaky（测试侧修复）

- **症状**：`loader_mtime_fence_sees_file_change` 全量并行负载下偶发失败（单跑稳定，10/10）。
- **根因**：两次连续 `fs::write` 间隔过短时，FS mtime 精度可能不足以区分（ext4 ns 级，但 overlay/tmpfs 表面 + 并行负载会弱化可见 delta）。原 `sleep 15ms` 在写 v2 **之后**，对 v2 自身 mtime 无帮助——它只保证写操作落盘，不保证 mtime 推进。
- **修复**：`memory/tests.rs::loader_mtime_fence_sees_file_change` 改为 **spin until mtime 真推进**（确定性，不依赖固定 sleep）：记 first_mtime → loop { 写 v2 + stat } 直到 mtime 变化，cap 2s，spin 失败 panic（FS 精度不足本身是 fence-invalidating bug）。
- **其他 mtime 测试不需改**：`hit_when_unchanged`（无第二次写）/ `sees_file_appear`（None≠Some 确定）/ `sees_file_vanish`（Some≠None 确定）都不依赖 mtime 精度。

### 10.3 commit 拆分（追加在 §4 之后）

- `fix(background-shell): drain race — wait for in-flight shell push (CI flaky)` —— 生产代码（in_memory.rs drain_notifications）
- `test(memory): spin until mtime advances in fence test (CI flaky)` —— 测试代码（tests.rs）
- 与 §4 的 fmt commit + workflow commit 同 PR。
