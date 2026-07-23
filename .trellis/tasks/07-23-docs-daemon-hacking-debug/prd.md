# PRD: docs sync C — Hacking/调试文档

> 父任务:`../07-23-docs-sync-daemon-split/`
> 审计依据:`../07-23-docs-sync-daemon-split/research/audit-daemon-docs-drift.md` §C1/C2/C3
> 跨任务一致性约定:见父 prd.md

## 目标

让环境踩坑笔记、LLM 配置笔记、SQLite 调试指引反映 daemon 化后的新运维现实:多进程 env 传递、DB catalog 优先于 env、`scripts/daemon.sh` 管理、daemon 视角 DB 路径。

## 范围

### C1. `docs/HACKING-llm.md`(严重过时)
- 默认模型名(行 13/14/145/198):`GLM-4.7` → `MiniMax-M2.7`(核对 `llm/provider/anthropic.rs:79`)
- env-var 主路径假设(行 11-20 / 140-156 / 196-204):新增「env vs DB catalog 优先级」章节 —— DB catalog(`providers` 表 + 加密 `api_key_enc`)是生产路径,`LlmConfig::from_env()` 仅冷启动兜底(state.rs:284 注释)。改 env 不一定切 provider
- checklist 第 1 项(行 144)vs 差异 5(行 425-461):base_url 约定内部不一致,补 OpenAI 路径(裸 host 不加 /v1)
- 新增:daemon 进程 env 传递机制(sidecar 继承 GUI env;裸跑继承 shell env)+ 交叉引用 DEBUG_DB §4 RULE-D-001

### C2. `docs/HACKING-wsl.md`
- §远程访问 daemon 部署(行 600-703):补 `scripts/daemon.sh` 用法 —— commit `a2bd611` 之后文档未同步。补:`./scripts/daemon.sh {start|bg|stop|restart|rebuild|status|logs}` + PID 文件管理 + 多实例保护警告(Q1 反模式:同时跑两个 daemon 导致数据分裂)
- 通用检查清单(行 558-592):整合 daemon 健康检查(`curl localhost:7456/api/v1/health` / `ss -tlnp | grep 7456` / `./scripts/daemon.sh status`)

### C3. `docs/DEBUG_DB.md`
- 路径表(行 13-19):✅ 已准确(16548fd 对齐),不改
- 源码引用行号(行 19):`state.rs:212-214` → 实际 `state.rs:304`(db_path) + daemon 侧 `bin/everlasting-daemon.rs:173-200`(`resolve_data_dir`)
- §1 DB 路径(行 9-30):补 daemon 视角三条解析路径 ① GUI `app_data_dir()` ② daemon `resolve_data_dir()` = `dirs::data_dir().join(EVERLASTING_APP_IDENTIFIER)` ③ sidecar `--data-dir` 显式传参 + 孤儿 DB 坑(commit 16548fd 之前的 `~/.local/share/everlasting/` 无 `dev.` 前缀)
- §4 WAL writer(行 161):「Tauri 进程持有 WAL writer」→ Thin 模式下 daemon 才是 WAL writer;`./scripts/daemon.sh stop` 才能安全直连写模式
- §5 reap_orphaned_runs(行 173):「app 启动时」→ 明确 daemon 启动时(Thin 模式 GUI 不调 load_inner)

## 验收标准

- [ ] HACKING-llm.md `GLM-4.7` 全文清零,改为 `MiniMax-M2.7`
- [ ] HACKING-llm.md 有「env vs DB catalog 优先级」章节 + daemon env 传递说明
- [ ] HACKING-llm.md base_url 约定 Anthropic/OpenAI 两条路径都覆盖
- [ ] HACKING-wsl.md daemon 部署章节含 `scripts/daemon.sh` 完整用法 + 多实例警告
- [ ] HACKING-wsl.md 通用检查清单含 daemon 健康检查项
- [ ] DEBUG_DB.md 源码行号修正
- [ ] DEBUG_DB.md 有 daemon 视角 DB 路径解析 + 孤儿 DB 坑说明
- [ ] DEBUG_DB.md WAL writer 归属修正(daemon 而非 Tauri 进程)
- [ ] 三份文档交叉引用完整(HACKING-llm ↔ DEBUG_DB RULE-D-001;HACKING-wsl ↔ scripts/daemon.sh)

## 风险

- HACKING-llm.md 是高频参考文档(38KB),改 env/catalog 优先级要确保与实际 `state.rs` load 逻辑 + provider/mod.rs build_provider 一致,先读这两处确认。
- DEBUG_DB 行号引用易二次漂移,改后用 grep 实测定位。
