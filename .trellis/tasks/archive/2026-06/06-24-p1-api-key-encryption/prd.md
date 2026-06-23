# brainstorm: API key 加密 (RULE-D-001)

> Trellis task `06-24-p1-api-key-encryption` · 来源 `.trellis/reviews/DEBT.md` §RULE-D-001 (P1)

## Goal

消除 provider api_key 明文存储与明文 IPC 回传的安全债。当前 DB `providers.api_key` 列以明文存全部 provider key,`list_providers` IPC 一次性把所有明文 key 回传前端。目标:DB 文件泄露 ≠ 全部 provider key 泄露;前端不再持有任意 provider 的明文 key(运行时发请求仍可解密/取回明文)。

## What I already know (已读代码确认)

### 明文存储链路
- **DB 层** `app/src-tauri/src/db/providers.rs`:`create_provider`/`list_providers`/`get_provider`/`update_provider` 全程明文 bind+read `api_key`;`ProviderRow` struct 含明文 `api_key` 字段
- **schema** `app/src-tauri/src/db/migrations.rs:240`:`api_key TEXT NOT NULL DEFAULT ''`
- **IPC 层** `app/src-tauri/src/commands/providers.rs`:`list_providers` 返回 `Vec<ProviderRow>`(含明文 apiKey);`add_provider`/`update_provider` 接收明文 api_key 入参;`test_model` 用 `provider.api_key` 发请求
- **前端** `app/src/stores/providers.ts`:`ProviderRow.apiKey: string` 明文贯穿 load/add/update
- **Settings UI** `app/src/components/settings/ProvidersTab.vue`:列表 `maskApiKey()`(前6字符+`****`)只做显示 mask;但编辑模式 `form.apiKey = p.apiKey`(:98)**回填明文到 input**

### 运行时读取(热路径)
- **唯一明文消费点**:`app/src-tauri/src/agent/provider.rs:107 resolve_chat_provider` → 读 provider_row → `:147` pre-flight 检查 `api_key.is_empty()` → 把明文 key 交给 provider factory 发 LLM 请求
- `app/src-tauri/src/agent/chat.rs:342` 同一 pre-flight(catalog fast-path)
- `test_model`(:367/:409)另用 `provider.api_key` 发探测请求
- → 解密点落点极集中:`resolve_chat_provider` + `test_model` 两处

### 依赖现状
- `Cargo.toml`:零加密/keyring 依赖(reqwest 用 rustls;已有 `dirs`/`libc`/`regex`)

## Assumptions (temporary, 待 research / 决策验证)
- api_key 必须**可逆还原**(运行时要发请求),不能用单向 hash
- 威胁模型:防的是"DB 文件 / app_data_dir 被整体泄露"场景,不是防"攻击者已在进程内存里"或"用户本机被 root"
- WSL2 + WSLg 是主开发/运行环境,Linux secret service 可用性是方案可行性的硬约束

## Open Questions (Blocking / Preference,待 research 收敛后逐个问)

1. ~~选型方向~~ ✅ **已决(2026-06-24)**:纯加密 MVP — Approach A(AES-GCM + machine-id),keyring/stronghold 进 Out of Scope。见 Decision #1
2. ~~前端回填 UX~~ ✅ **已决(2026-06-24)**:留空覆盖。`list_providers` 改返 `hasKey` 布尔(不返明文 apiKey);`update_provider` 区分"未传 key=保持不变"。见 Decision #2
3. ~~明文迁移~~ → 研究已定:加新列 `api_key_enc` + `key_migrated_at` 哨兵,启动幂等迁移(读旧明文→加密写新列→同事务抹除旧明文),复用 `add_*_column_if_missing` + 幂等 UPDATE 模式(`migrations.rs:654-690`)
4. ~~master key 来源~~ → 研究已定:`machine-uid` crate 取 machine-id + HKDF(Sha256) 派生 master key(方案 b;排除 a 编译期固定 / c OS keystore 回流 keyring / d 用户密码 UX 灾难)
5. ~~解密时机~~ → 现有架构已定:`resolve_chat_provider` 每次从 DB 读 `api_key_enc` 现解密(`provider.rs:131` 注释:catalog 缓存 model/protocol 但 NOT api_key),明文短暂存在于请求周期,不缓存明文进内存

## Requirements

**存储**
- 新增 `crypto.rs` 模块:AES-256-GCM(`aes-gcm`) + HKDF(Sha256,`hkdf`) 从 `machine-uid` 取 machine-id 派生 master key;密文 `blob = VERSION(1B)||nonce(12B)||ct||tag(16B)` base64
- **AAD 关联数据**:加密时以 provider id 作 AAD,密文与 provider 绑定(防 DB 内密文被挪到别的 provider 行解密成功)
- DB 加列 `api_key_enc TEXT`(密文)+ `key_migrated_at TEXT`(哨兵);复用 `add_*_column_if_missing` 模式

**迁移**
- 启动幂等迁移:检测 `api_key != '' AND key_migrated_at IS NULL` → 加密写 `api_key_enc` + 同事务抹除 `api_key`(置 '')+ 写哨兵;崩溃重启靠哨兵可续;空 key 跳过

**运行时解密**
- `resolve_chat_provider` 从 DB 读 `api_key_enc` 现解密(provider id 作 AAD)→ 明文给 provider factory;`test_model` 同链路
- **解密失败兜底**:解密失败(机器变化/损坏)不 panic,降级返回友好错误引导到 Settings 重粘 key

**IPC / 前端(Decision #2)**
- `list_providers` 响应以 `hasKey: boolean` 替代明文 `apiKey`
- `update_provider` 新可选语义:未传 apiKey=保持原 key,传值=覆盖
- 前端 `ProviderRow` type:`apiKey: string` → `hasKey: boolean`;ProvidersTab 编辑留空覆盖 + placeholder + **加密徽标 UI**
- 新增 provider 走加密存储

## Acceptance Criteria
- [ ] DB `providers.api_key` 列迁移后全为空(明文已抹除),密文在 `api_key_enc`;`strings`/hexdump DB 文件不可见明文 key 模式
- [ ] `list_providers` IPC 响应不含任意 provider 明文 apiKey(只有 hasKey 布尔)
- [ ] 现有 provider 配 chat 仍能正常发请求(`resolve_chat_provider` 解密链路通);`test_model` 同
- [ ] 启动一次后旧明文 key 迁移并抹除;再启动幂等(哨兵 + 不重复迁移);崩溃重启可续
- [ ] AAD 防护:把 provider A 密文挪到 provider B 行 → 解密失败(单测 mismatch)
- [ ] machine-id 变化(密文用旧 master key)→ 解密失败降级友好提示,不 panic
- [ ] 单测:加解密 roundtrip / 空 key / tamper / AAD mismatch / 迁移幂等
- [ ] `cargo test` + vitest + `cargo check` 0 warning;ProvidersTab 留空覆盖 + 徽标 UI 验证
- [ ] DEBT.md §RULE-D-001 PR merge 后删除(四段式 commit)

## Definition of Done
- 单元测试覆盖:加解密往返、迁移幂等、空 key 边界
- `cargo test` + vitest + `cargo check` 0 warning
- DEBT.md §RULE-D-001 在 PR merge 后删除(走 trellis 四段式 commit)
- spec/backend 若涉及 wire 形态变化(provider row)同步更新

## Out of Scope (explicit)
- master key 硬件级保护(TPM/Secure Enclave)
- 防本机 root / 进程内存窃取(超出威胁模型)
- 前端 key 端到端加密传输(Tauri 本地通道,非网络)
- keyring(双路径 B)/ stronghold(C) — WSL 主环境不可用 + 过度工程,macOS/Win 后置增强
- **迁移前 DB 自动备份**(本次未选) — 依赖迁移幂等 + 哨兵兜底
- master key 轮换命令(machine-id 变化时批量重加密) — 未来增强

## Research References (三方调研交叉验证,结论收敛)

- [x] [`research/keyring-wsl-availability.md`](research/keyring-wsl-availability.md) — **WSL 实测 keyring 开箱不可用**(本机无 gnome-keyring/libsecret/secret-service;keyutils kernel 后端重启即丢不适合长期凭证)→ keyring 主方案被否
- [x] [`research/industry-api-key-storage.md`](research/industry-api-key-storage.md) — 业界同类工具(Codex CLI/Claude Code/Aider/Continue)默认全走明文文件/env var,Codex 提供 keyring 仍默认 file → 本项目加密后已比主流更安全
- [x] [`research/app-layer-encryption-rust.md`](research/app-layer-encryption-rust.md) — 推荐 `aes-gcm`(Aes256Gcm)+`hkdf`(Sha256)从 machine-id 派生 master key,5 直接依赖,密文 `VERSION||nonce||ct||tag` base64 进 SQLite

> 决定性一手数据点:本机 `/etc/machine-id` 存在且稳定(`b320623d...`),`gnome-keyring` 系列全未装 → 选 machine-id 加密而非 keyring。

## Research Notes / Feasible Approaches

### What similar tools do
- Codex CLI:`cli_auth_credentials_store = file|keyring|auto`,**默认 file**(最强参照证据)
- Claude Code / Aider / Continue / OpenAI SDK:默认 env var 或明文 JSON config,均不上 OS keyring
- → 加密(哪怕 machine-id 派生)已超出业界主流安全水位

### Constraints from our repo
- WSL2 主环境 `gnome-keyring`/`libsecret`/`secret-service` **全未安装** → keyring 在主环境不可用
- `/etc/machine-id` 稳定存在 → machine-id 派生 master key 可行
- DB migration 体系已有 `add_*_column_if_missing` + 幂等 UPDATE 回填模式(`migrations.rs:654-690`)
- 威胁模型只防"DB 文件整体泄露",不防本机 root / 进程内存

### Feasible approaches

**Approach A: AES-256-GCM + machine-id 派生 master key (Recommended ✅)**
- How:`aes-gcm`(Aes256Gcm)+`hkdf`(Sha256) 从 `machine-uid` crate 取 machine-id 派生 master key;每条 key 随机 nonce;密文 `blob = VERSION(1B)||nonce(12B)||ct||tag(16B)` base64 存新列 `api_key_enc`;`add_*_column_if_missing` + 启动幂等迁移(读旧明文→加密写新列→抹除旧明文)
- Pros:WSL 零摩擦;5 直接依赖(aes-gcm/hkdf/sha2/machine-uid/base64);命中威胁模型(DB 泄露但无 machine-id 解不开);业界主流之上;跨平台一致
- Cons:机器绑定固有性质 — `wsl --unregister`/重装发行班重置 machine-id → 旧密文不可解密(需 UX 提示重粘 key);不防本机 root

**Approach B: keyring + 加密 fallback(双路径)**
- How:macOS/Win 用 OS keystore,WSL/Linux 检测无 secret service 时降级到 Approach A
- Pros:macOS/Win 用最强原生 keystore
- Cons:WSL 是主环境 = keyring 分支在主环境是死代码,纯增复杂度;两条路径要测;macOS/Win 是极少数场景(个人 WSL-first)

**Approach C: tauri-plugin-stronghold**
- How:IOTA Stronghold 本地加密 vault
- Cons:过度工程(Stronghold 为高安全场景设计),依赖重,个人 app 不值得

### 初步决策倾向
Approach A(MVP 纯加密),keyring( Approach B 的 macOS/Win 分支) + stronghold( C) 进 Out of Scope。**待用户确认 MVP 范围**(见下 Q)。

## Decision (ADR-lite) #1 — 选型 (2026-06-24)

**Context**: provider api_key 明文存 DB + 明文 IPC 回传(P1 安全债 RULE-D-001)。备选 keyring / 应用层加密 / stronghold。
**Decision**: 采用 **Approach A — AES-256-GCM + machine-id(`/etc/machine-id`)派生 master key**,纯加密 MVP。keyring(双路径 B)+ stronghold(C)进 Out of Scope。
**理由**:
- keyring 在 WSL 主环境实测开箱不可用(无 gnome-keyring/secret-service;keyutils 后端重启即丢)→ 双路径里 WSL 永远走加密分支,keyring 代码在主环境是死代码
- 业界同类工具(Codex CLI/Claude Code/Aider/Continue)默认明文文件/env var,加密已超主流水位
- machine-id 加密命中威胁模型(DB 泄露但无 machine-id 解不开),WSL 零摩擦,5 直接依赖
**Consequences / Risks**:
- 机器绑定固有性质:`wsl --unregister`/重装发行班重置 machine-id → 旧密文不可解密,需 UX 提示重粘 key
- 不防本机 root / 进程内存窃取(超出威胁模型,Out of Scope)
- macOS/Win 也用同一 `machine-uid` crate(machine-id 跨平台派生),未来要更强可后置 keyring 增强

## Decision (ADR-lite) #2 — 前端不持明文 key (2026-06-24)

**Context**: RULE-D-001 要求 `list_providers` IPC 不再回传明文 apiKey;前端编辑当前回填明文(`ProvidersTab.vue:98`)。
**Decision**: 留空覆盖 UX。apiKey input 永不回填明文 + placeholder 提示;`list_providers` 响应以 `hasKey` 布尔替代明文 apiKey;`update_provider` 区分"未传 apiKey=保持原 key"vs"传新值=覆盖"。
**理由**: 彻底切断前端持明文 key 的路径,RULE-D-001 收益最大化;secret 输入业界标准 UX;前端 `ProviderRow.apiKey: string` 字段语义变 `hasKey: boolean`。
**Consequences**: 用户编辑 provider 看不到完整 key(只能覆盖,不能查看)— 可接受(secret 性质);`update_provider` 需新可选参数语义;前端 store + ProvidersTab + type 同步改。

## Technical Approach

### 加密模块 `crypto.rs`(新增)
- `derive_master_key() -> [u8;32]`:`machine-uid::get()` + HKDF(Sha256, salt=固定 app tag, info=b"everlasting/api-key/v1")
- `encrypt(plaintext, aad: provider_id) -> Result<String>`:OsRng nonce(12B) + Aes256Gcm seal(aad=provider_id) → `VERSION||nonce||ct||tag` base64
- `decrypt(blob, aad) -> Result<String>`:逆操作,AAD mismatch/tamper 错误冒泡
- 单测:roundtrip / 空 plaintext / tamper / AAD mismatch

### DB 层(`db/migrations.rs` + `db/providers.rs`)
- migration:`add_*_column_if_missing` 加 `api_key_enc` + `key_migrated_at`
- `create_provider`/`update_provider`:明文 key → `encrypt` 写 `api_key_enc`;旧 `api_key` 列不再写入(留空)
- `get_provider`/`list_providers`:返回不含明文(list 仅 hasKey);runtime 路径提供 `get_provider_encrypted_key(id)` 供解密
- `update_provider`:apiKey 入参改 `Option<String>` — None=保持(不动 api_key_enc),Some=覆盖加密

### 运行时(`agent/provider.rs` + `commands/providers.rs`)
- `resolve_chat_provider`:读 `api_key_enc` → `decrypt(blob, provider_id)` → 明文给 factory;解密失败降级 PreFlightError 友好文案(引导 Settings 重粘)
- `test_model`:同链路取 key
- **启动迁移**(migrations 或独立 migrate 函数):扫 `api_key != '' AND key_migrated_at IS NULL` → 逐条 encrypt + 抹除 + 写哨兵

### 前端(`stores/providers.ts` + `ProvidersTab.vue`)
- type `ProviderRow.apiKey: string` → `hasKey: boolean`
- ProvidersTab 编辑:apiKey input 留空覆盖 + placeholder + "已加密保存"徽标
- update 调用:空 input → 不传 apiKey(undefined) → 后端保持

## Implementation Plan (small PRs)

- **PR1** — `crypto.rs` 模块 + 5 依赖(`aes-gcm`/`hkdf`/`sha2`/`machine-uid`/`base64`)+ 单测(roundtrip/empty/tamper/AAD);纯库,无 wire 变化;`cargo check` 0 warning
- **PR2** — DB migration(`api_key_enc`+`key_migrated_at`)+ 启动幂等迁移 + 抹除旧明文 + 迁移幂等单测;后端内部(IPC 暂双写兼容)
- **PR3** — 运行时解密接通 `resolve_chat_provider` + `test_model` + 解密失败兜底;移除 `api_key` 明文兼容读
- **PR4** — IPC wire 变更:`list_providers` 返 hasKey;`update_provider` Option<apiKey> 语义;前端 type + ProvidersTab 留空覆盖 + 加密徽标 + vitest
- **PR5** — DEBT.md 删 RULE-D-001 + spec/backend wire 同步 + 收尾(四段式 commit:fix→docs(debt)→archive→journal)

## Technical Notes
- 运行时解密落点候选:`agent/provider.rs resolve_chat_provider` + `commands/providers.rs test_model`
- DB migration 体系:`db/migrations.rs`(参考现有 v6 migration 的幂等 UPDATE 模式,见 ROADMAP Mode 3 档化)
- IPC wire:`ProviderRow` 是 camelCase,若 apiKey 字段语义变化需同步前端 type
- 相关 spec:`.trellis/spec/backend/`(provider/llm-contract 待查)
