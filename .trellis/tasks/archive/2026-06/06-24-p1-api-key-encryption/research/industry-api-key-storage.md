# Research: 业界同类工具 / Tauri 生态如何存储 API key

- **Query**: 调研业界同类工具(coding agent + Tauri 2 生态)如何存 LLM API key,为本项目(Rust + Tauri 2 桌面 coding agent,WSL-first,个人单用户,已有 SQLite)选型提供依据
- **Scope**: external(官方 docs / GitHub README / docs.rs)
- **Date**: 2026-06-24
- **关联任务**: `.trellis/tasks/06-24-p1-api-key-encryption/prd.md`(RULE-D-001,P1)
- **关联 research**: `research/keyring-wsl-availability.md`(本文件给出 WSL 一手验证数据点)

---

## 结论先行

### 主流做法(分两层)

**第 1 层:同类 coding agent / CLI 工具——几乎所有都不上 OS keyring 作默认,走"明文文件 + env var"。**

| 工具 | 默认存哪 | 是否 keyring | 关键证据 |
|---|---|---|---|
| **Codex CLI**(OpenAI,Rust) | `~/.codex/auth.json`(明文,ChatGPT OAuth token / API key) | 否(默认 `auto`→多数落 file) | 官方 auth 页明确:`cli_auth_credentials_store = file\|keyring\|auto`,**默认 file**;明文 `auth.json` 警告"treat it like a password" |
| **Claude Code CLI**(Anthropic) | env var `ANTHROPIC_API_KEY`(`export`),不持久化到自有配置 | 否 | 见本项目 `llm/provider/anthropic.rs:32` `std::env::var("ANTHROPIC_API_KEY")`,沿用 Anthropic SDK 约定 |
| **OpenAI 官方 SDK**(Python/Node) | env var `OPENAI_API_KEY`,配合 `.env` + `python-dotenv` | 否 | openai-python README: `api_key=os.environ.get("OPENAI_API_KEY")`,显式建议 dotenv"not stored in source control" |
| **Aider** | env var / `~/.aider.conf.yml`(明文) | 否 | Aider docs config/dotenv: `.env` 文件读 env var |
| **Continue.dev**(VSCode 扩展) | `~/.continue/config.json`(明文,内含 `apiKey`) | 否 | Continue 默认明文 JSON config |
| **Cursor**(闭源 IDE) | 内置账号/OAuth + 明文 settings | 否(账号态为主) | 闭源,无公开 keyring 证据 |
| **Cline**(VSCode 扩展) | **VSCode SecretStorage API**(扩展宿主的加密存储) | 是(借宿主) | Cline 把 apiKey 存进 VSCode `context.secrets`,即 VSCode 底层走各平台 keychain |

**关键观察**:业界 coding 工具默认**不上 OS keyring**——要么明文文件、要么 env var,把"key 不进 git"和"文件权限 0600"当成事实安全边界。Codex CLI 是唯一显式提供 `keyring` 选项的同类,且**默认仍走 file**。唯一例外是 Cline 这类 VSCode 扩展,它"免费蹭"宿主的 SecretStorage,自己不用实现 keyring 集成。

### 对本项目的推荐方向(详见末尾 Feasible 方案)

**推荐主方案:应用层对称加密(AES-256-GCM)+ 机器绑定 master key(`/etc/machine-id` / Windows MachineGuid / macOS IOPlatformUUID + HKDF-SHA256)**,加密后写回 SQLite `providers.api_key` 列。

理由(针对本项目约束):
1. **WSL-first 的硬约束**:本机实测 `gnome-keyring` / `libsecret` / `secret-service` **全未安装**(见下方 WSL 数据点),WSLg 默认环境跑不起来 Secret Service D-Bus 接口 → **keyring crate 在 WSL 上有可用性生死线**。机器绑定加密不依赖任何外部 daemon。
2. **威胁模型吻合**:PRD 明确"防 DB 文件 / app_data_dir 整体泄露,不防本机 root / 进程内存"。AES-GCM + machine-id 派生 key 正好命中这一层:DB 被拷走 → 在另一台机器上解不开。
3. **零运行时交互**:个人单用户、无登录流程,master key 由 machine-id 自动派生,无需用户输密码,UX 等同现状。
4. **依赖极轻**:加 `aes-gcm` + `hkdf` + `sha2` + `dirs` 即可,不动 Tauri 插件体系,不引入 Stronghold/argon2 的重依赖。
5. **解密点集中**:PRD 已确认只有 `agent/provider.rs:resolve_chat_provider` + `commands/providers.rs:test_model` 两处消费明文 key,加解密落点清晰。

**次选 / fallback 方案**:`keyring` crate(Linux 走 `dbus-secret-service` 或 `linux-keyutils`,macOS Keychain,Windows Credential Manager)+ DB 存"空/哨兵值"。但需接受 WSL 上要么引导用户装 `gnome-keyring`,要么降级到加密方案。

**明确不推荐**:`tauri-plugin-stronghold`(IOTA Stronghold)。它是为"隔离 secrets 免受进程级泄漏"设计(encrypted snapshot + in-memory vault + SLIP-10 ed25519),威胁模型高于本项目需求;且**需要用户密码或固定 password hash function 解锁 vault**,与"个人单用户无登录"UX 冲突。详见对比表。

---

## 必查项 1:同类 coding agent 工具存 key 方式(证据)

### Codex CLI(OpenAI,Rust)— 最相关参照

**官方 auth 页**(`https://developers.openai.com/codex/auth`)原文要点:

- 默认登录 = ChatGPT OAuth;也可"Sign in with an API key"。
- **`Credential storage` 一节**给出 `cli_auth_credentials_store` 配置三档:
  - `file`(默认行为)→ 明文写 `~/.codex/auth.json`
  - `keyring` → 写 OS credential store
  - `auto` → 优先 OS credential store,不可用则 fallback 到 `auth.json`
- 官方对明文 file 的告诫:"treat `~/.codex/auth.json` like a password"(即承认明文,只靠文件权限兜底)。
- 远程 / headless 环境:可 `codex login --with-access-token` 注入,或拷贝已缓存凭据。

**对本项目的启示**:Codex 默认就是明文 file;它把"要不要上 keyring"做成**用户可选项**(`auto` 兜底)。这是最务实的范式:**默认不依赖 keyring,把它作为 nice-to-have**。本项目 WSL 上 keyring 不可用,正好对应 Codex 的 `auto`→`file` fallback 路径——但本项目可以做得**比明文 file 更好**:加一层机器绑定加密。

### Claude Code CLI(Anthropic)

- 纯 env var:`ANTHROPIC_API_KEY`(`export` 后由 SDK 读取)。
- 无自有持久化凭据文件;`login` 流程走 OAuth(`claude setup-token` 等),token 存在 `~/.claude.json` / keychain(macOS 上 Claude Code 会用 Keychain)。
- **本项目现状完全沿用此约定**:`app/src-tauri/src/llm/provider/anthropic.rs:32` `std::env::var("ANTHROPIC_API_KEY")`。

### OpenAI 官方 SDK(Python / Node)

- 官方 README 明确:`api_key=os.environ.get("OPENAI_API_KEY")`,推荐 `python-dotenv` + `.env`,理由"so that your API key is not stored in source control"。
- 即:**官方推荐 env var + .env 文件(明文)**,不碰 keyring。

### Aider

- env var(`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` 等)或 `~/.aider.conf.yml`(明文 YAML,可写 `openai-api-key: sk-...`)。
- 无 keyring 集成。

### Continue.dev

- `~/.continue/config.json`(明文 JSON,`models[].apiKey` 直接明文)。
- 企业版有 Continue Hub 走代理(避免本地存 key),但本地配置仍是明文。

### Cursor(闭源)

- 主推 Cursor 账号登录(服务端代持 key);本地仅存 session/OAuth token。
- 也可在 settings 里填 provider key(明文配置)。闭源,无公开 keyring 行为。

### Cline(VSCode 扩展)— 唯一 keyring 派

- 把每个 provider 的 apiKey 存进 **VSCode `context.secrets` API**(即 `SecretStorage`),由 VSCode 宿主转发到底层 OS keychain(macOS Keychain / Windows Credential Manager / Linux libsecret)。
- **代价**:Cline 自己不用实现跨平台 keyring,但**强绑定 VSCode 宿主**——本项目是独立 Tauri app,无此"免费宿主",所以这条路对 Tauri 不成立。

---

## 必查项 2:Tauri 2 生态的 secret 存储方案(证据 + trade-off)

### `tauri-plugin-stronghold`(IOTA Stronghold 加密 vault)

**官方 docs**(`https://v2.tauri.app/plugin/stronghold/`)+ plugin README(`plugins-workspace/v2/plugins/stronghold`)要点:

- 基于 [IOTA Stronghold](https://github.com/iotaledger/stronghold.rs):"isolating digital secrets from exposure to hackers and accidental leaks",架构是 **encrypted snapshot 文件 + in-memory Vault**。
- 平台支持表(README):Linux / Windows / macOS / Android / iOS 全 ✓。
- **必须传 password hash function**(32 字节,Stronghold 硬约束),官方示例用 `argon2id`(mem_cost=10000, time_cost=10)。也提供 `Builder::with_argon2(&salt_path)` 默认实现,salt 存 `app_local_data_dir/salt.txt`。
- Stronghold README 自述目标场景含 "Works with Yubikey" / "Secure Element to generate private keys"——**面向比"防 DB 泄漏"更高级别威胁**。
- 已知坑(README 提示):upstream bug `#2048`,需在 `Cargo.toml` 加 `[profile.dev.package.scrypt] opt-level = 3`(否则 dev 构建慢)。

**trade-off**:
- (+) 行业级安全隔离,进程内存泄漏也防。
- (+) 有现成 Tauri plugin,JS guest binding 可直接调。
- (−) **必须有 password / 密码派生 key 来 unlock vault**——与"个人单用户、无登录、开机即用"UX 直接冲突。固定 password = 退化为"编译期/硬编码 salt 派生 key",安全收益不比直接 machine-id 派生高。
- (−) 依赖重(stronghold-rs 全家桶 + argon2 + scrypt),拉长编译、增大 binary。
- (−) 加密粒度是"整个 vault",存一个 API key 是杀鸡用牛刀。

### `tauri-plugin-store`(明文 JSON)

- plugin README 自述:"Simple, persistent key-value store"。五平台全 ✓。
- 实现就是 `app_data_dir` 下的 JSON 文件,**明文**。
- **对本项目安全目标无用**(DB 已是明文,再换个明文 JSON 没区别)。仅适合存非敏感配置(主题、窗口位置)。

### 直接用 `keyring` crate(无 Tauri 封装)

**docs.rs `keyring` 4.1.2**(`https://docs.rs/keyring/latest/keyring/`)依赖树给出 Linux 后端三选一:
- `dbus-secret-service-keyring-store` ^1.0.0(D-Bus 调 FreeDesktop Secret Service,即 `gnome-keyring` / `kwallet`)
- `zbus-secret-service-keyring-store` ^1.0.0(纯 Rust zbus 实现 Secret Service)
- `linux-keyutils-keyring-store` ^1.0.0(内核 keyrings,session 级,重启丢)

**架构(README)**:从 v2 起 `keyring` 拆成 `keyring-core` + 每个 platform 的 credential store crate,按需 feature 选。`v1` feature 保持向后兼容(自动选默认 store)。

**trade-off**:
- (+) 跨平台 API 一致,代码简单 `Entry::new(service, user)?.set_password(...)`。
- (+) macOS Keychain / Windows Credential Manager 开箱即用,用户无感。
- (−) **Linux/WSL 硬依赖 Secret Service daemon**(见下方 WSL 数据点)。
- (−) secret 粒度是"一个 entry 一个 secret",多 provider key = 多 entry,管理略碎。
- (−) Linux 上 `linux-keyutils` 后端是 session 级(重启丢失),不适合持久化 provider key。

---

## 必查项 3:master key 来源的主流实践

当走"应用层对称加密"路线,master key 必须有个来源。主流四种:

| 方案 | 可逆性/强度 | WSL 可用 | UX 代价 | 备注 |
|---|---|---|---|---|
| **机器绑定派生**(machine-id / MAC + HKDF/argon2) | 中(同机可还原,换机失效) | **✓**(WSL 有 `/etc/machine-id`) | 0(全自动) | **本项目推荐**。命中"防 DB 文件拷走"威胁模型 |
| OS keystore 存 master key | 高 | ✗(WSL 无 keyring daemon) | 低 | 又回到 keyring 依赖,Wg 死锁;适合有 keychain 的纯 macOS/Win app |
| 用户密码(PBKDF2/argon2 派生) | 高(用户脑中) | ✓ | **高**(每次启动输密码 / 维持 session unlock) | 个人单用户、无登录流程 → 不值得 |
| 编译期固定 key(硬编码) | 低(逆向 binary 可提取) | ✓ | 0 | 等于没加密,只挡脚本小子;不建议 |

### 机器绑定派生的细节要点

- **Linux**:`/etc/machine-id`(systemd 生成,32 hex 字符,跨重启稳定,除非重建系统)。本机实测:`b320623da08d4a47a81095768727b444`(33 字节含换行,实际 32 hex)。`/var/lib/dbus/machine-id` 是它的 symlink。
- **Windows**:`HKLM\Software\Microsoft\Cryptography\MachineGuid`(注册表,装机生成)。
- **macOS**:`IOPlatformUUID`(`ioreg -d2 -c IOPlatformExpertDevice`)。
- 派生:读出平台 ID(UTF-8 bytes)→ HKDF-SHA256(salt = app-specific constant, info = "everlasting-master-key")→ 32 字节 AES key。
- **strength**:挡"DB 文件被离线拷走"(换机无 machine-id → 解不开);**不挡**"同机攻击者"(他能读 `/etc/machine-id`,自然能解密)——与 PRD 威胁模型一致。

### OS keystore 存 master key(混合方案)

- 思路:keyring 存 master key,secret 用 master key 加密后存 DB。
- 好处:secret 量大时 keyring 不必存 N 条;只存 1 条 master。
- 坏处:**WSL 上 keyring 不可用 → 整个方案不可用**,除非再加 fallback。对本项目不如直接 machine-id 派生简单。

---

## 必查项 4:个人单用户本地 app 的取舍

**核心判断:Stronghold 这类重方案不值得;machine-id 加密 或 keyring(若平台支持) 即可。**

| 维度 | machine-id 加密(主推) | keyring crate | Stronghold |
|---|---|---|---|
| 实现复杂度 | 低(2 个 crate,~100 行) | 中(跨平台 + Linux fallback) | 高(plugin + argon2 + vault API) |
| WSL 可用 | **✓** | ✗(需装 daemon) | △(可用但 password 来源同问题) |
| UX(个人单用户) | 零摩擦 | 零摩擦(平台有 keychain 时) | 需 unlock 步骤 |
| 挡 DB 文件泄露 | **✓** | ✓(DB 里不存 secret) | ✓ |
| 挡同机攻击者 | ✗(与 PRD 一致) | △ | ✓ |
| 依赖重量 | 极轻(aes-gcm+hkdf+sha2) | 中(dbus/zbus) | 重(stronghold-rs) |
| 是否匹配 PRD 威胁模型 | **精确匹配** | 匹配 | 过度匹配 |

**结论**:对"个人单用户、WSL-first、防 DB 泄露"这三条,Stronghold 的额外安全收益(防进程内存泄漏)落在 PRD **Out of Scope** 里("防本机 root / 进程内存窃取"已明确排除)。机器绑定加密是精确解,keyring 是可选增强。

---

## 对本项目的明确推荐(2-3 个 feasible 方案)

### 方案 A(主推):AES-256-GCM + machine-id 派生 master key

- **存哪**:加密 ciphertext(含 nonce + tag)写回 SQLite `providers.api_key` 列(改 schema 或加 `api_key_nonce` 列)。
- **master key**:启动时从平台 machine-id 派生(HKDF-SHA256,salt 用 app 常量),缓存在 `AppState`(进程内存,不落盘)。
- **解密点**:`agent/provider.rs::resolve_chat_provider`(`:107`) + `commands/providers.rs::test_model`(`:367/:409`)两处现解密。
- **IPC 改造**:`list_providers` 不再回传明文 apiKey(返回掩码或空);Settings 编辑改"留空=保持,输入新值=覆盖"(业界 secret input 标准 UX,见 PRD Open Question 2)。
- **迁移**:db migration 加 `api_key_encrypted` 列 + 幂等 flag,启动一次性"读旧明文 → 加密写新列 → 抹旧明文"(参考现有 v6 migration 的幂等 UPDATE 模式)。
- **依赖**:`aes-gcm`,`hkdf`,`sha2`,`dirs`(已有)。无 Tauri 插件改动。
- **威胁模型匹配**:DB 被拷走 → 换机无 machine-id → 解不开。✓

### 方案 B(fallback / 可与 A 共存):keyring crate + DB 存哨兵

- **存哪**:provider key 进 OS keychain(`keyring::Entry::new("everlasting", provider_id)`),DB `api_key` 列存哨兵值(如空串或 `"__in_keyring__"` 标记)。
- **平台**:macOS Keychain / Windows Credential Manager 直接可用;Linux 用 `dbus-secret-service`(需 `gnome-keyring` 或 `kwallet`)。
- **WSL 现实**:本机 `gnome-keyring` / `libsecret` **未安装** → WSL 上自动 fallback 到方案 A(或提示用户装 `gnome-keyring`)。
- **价值**:macOS / Windows 用户拿到"OS 级保护"(DB 泄露也无法解密,因为 secret 根本不在 DB 里),比方案 A 更强。
- **复杂度**:跨平台 fallback 逻辑 + 启动探测 keychain 可用性。

### 方案 C(明确不推荐):tauri-plugin-stronghold

- 仅当未来威胁模型升级到"防进程内存泄漏 / 防本机非 root 攻击者"时再考虑。
- 当前 PRD Out of Scope 已排除此类威胁 → 现在上 Stronghold 是过度工程。

### 实施建议(给主 agent)

1. **选方案 A 作为 MVP**(精准命中 PRD,WSL 零摩擦,依赖最轻)。
2. **保留方案 B 作为后续增强**(macOS/Windows 用户体验更好),但 WSL 默认走 A。
3. **不做方案 C**(写进 PRD Out of Scope,避免后续反复讨论)。

---

## 外部引用清单

| 来源 | URL | 用途 |
|---|---|---|
| Codex CLI auth 官方 | https://developers.openai.com/codex/auth | `cli_auth_credentials_store = file\|keyring\|auto`,默认 file |
| Codex CLI README | https://github.com/openai/codex (README.md) | ChatGPT OAuth 默认 / API key 为备选 |
| OpenAI Python SDK README | https://github.com/openai/openai-python | env var `OPENAI_API_KEY` + dotenv 推荐 |
| Tauri Stronghold plugin docs | https://v2.tauri.app/plugin/stronghold/ | password hash function 必填(argon2 默认) |
| Tauri Stronghold plugin README | https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/stronghold/README.md | 五平台支持 / `#2048` scrypt opt-level 坑 |
| Tauri Store plugin README | https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/store/README.md | "Simple persistent kv",明文 |
| IOTA Stronghold README | https://github.com/iotaledger/stronghold-rs | encrypted snapshot + in-memory vault 定位 |
| keyring crate docs.rs | https://docs.rs/keyring/latest/keyring/ | Linux 后端三选一(dbus-secret-service / zbus-secret-service / linux-keyutils) |
| keyring-rs README | https://github.com/open-source-cooperative/keyring-rs | v2 拆 core + per-platform store,有 Tauri 2 demo GUI |

---

## Caveats / 未覆盖

- **未实际 fetch Aider / Continue / Cursor 的最新配置文档原文**:基于既有知识 + 部分 README grep,细节(如 Continue 是否新版加了 keychain 选项)建议主 agent 在落地方案前按需复核。但对选型结论(默认明文 / env var)无影响。
- **`keyring` crate 的最新版本号**:docs.rs 显示 4.1.2(截至 fetch);具体 feature flag 组合(`apple-native` / `windows-native` / `sync-secret-service`)需在 `cargo add` 时确认。
- **`linux-keyutils` 后端是否适合持久 provider key**:kernel keyring 有 session/ueyrling 等类型,默认 user session 级,重启丢——**不适合存 provider key**,只适合临时缓存。主 agent 选 keyring 后端时应排除它,只用 `dbus-secret-service` / `zbus-secret-service`。
- **machine-id 在容器 / 多 WSL 发行版的稳定性**:同一 Windows 宿主下不同 WSL 发行版 `/etc/machine-id` 可能不同(各自独立)→ 换发行版 = key 解不开。对本项目"个人单用户单机"场景可接受,但需在迁移逻辑里做幂等 + 容错。
- **未验证 Windows MachineGuid / macOS IOPlatformUUID 的具体读取 API**(本项目 WSL-first,先验证 Linux 路径;其他平台后续 PR 再补)。
