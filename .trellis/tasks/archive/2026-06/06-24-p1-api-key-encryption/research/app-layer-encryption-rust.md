# Research: Rust 应用层对称加密方案 (RULE-D-001 fallback 主方案)

- **Query**: 调研 Rust 应用层对称加密可行方案,作为 keyring 在 WSL 不可用时的 fallback 主方案
- **Scope**: external (crate 选型 / API / 安全边界) + 内部接入点 (现有 migration 体系 / Cargo.toml)
- **Date**: 2026-06-24
- **Threat model**: 防"DB 文件 / app_data_dir 整体泄露";不防本机 root / 进程内存窃取

---

## 结论先行 (TL;DR)

1. **推荐 crate**:`aes-gcm`(`Aes256Gcm`)。理由:RustCrypto 官方 AEADs 仓 / NCC Group 安全审计无重大发现 / 1.14 亿次下载(同类最多) / x86 上有 AES-NI 硬件加速 / API 与 `aead` trait 一致 / 与 `chacha20poly1305` 同源同 API,二选一即可,本场景选 AES-GCM(更广部署、硬件加速)。
2. **master key 来源**:机器绑定派生 = `machine-uid`(读 `/etc/machine-id` / `IOPlatformUUID` / `MachineGuid`)作 IKM → `hkdf`(Sha256)派生 32B master key,常驻进程内存。**不用 OS keystore**(回流 keyring WSL 问题)、**不用编译期固定 key**(反编译即得)、**不用用户密码**(UX 差)。
3. **能否防住 DB 泄露**:**能**(在威胁模型内)。攻击者只拿到 DB 文件 → 没有 machine-id → 无法派生 master key → 无法解密 `api_key` 密文。**防不住**:整台机器(含 machine-id)被克隆;或攻击者拿到运行中的进程内存。这两者明确在 Out of Scope。
4. **WSL 可行性 ✅**:本机实测 `/etc/machine-id` 存在(32 hex char,与 `/var/lib/dbus/machine-id` 一致);`getrandom` Linux syscall(即 `OsRng` → nonce 生成)在 WSL 原生可用。
5. **最小依赖增量**:`aes-gcm` + `hkdf` + `sha2` + `machine-uid` + `base64`。`aead`/`rand_core` 由 `aes-gcm` re-export,**不需要**单独加 `rand`/`getrandom`。

---

## Findings

### 1. crate 选型:`aes-gcm` vs `chacha20poly1305`

#### 成熟度对比 (crates.io 元数据,2026-06-24 拉取)

| crate | 最新版本 | 累计下载 | 近期下载(90d) | 维护方 | 安全审计 |
|---|---|---|---|---|---|
| **aes-gcm** | 0.10.3 (rc: 0.11.0-rc.4) | 114,637,494 | 25,081,014 | RustCrypto/AEADs | NCC Group 审计 ✅ 无重大发现 |
| **chacha20poly1305** | (rc: 0.11.0-rc.3) | 62,201,063 | 12,897,049 | RustCrypto/AEADs | 同一份 NCC Group 审计 ✅ |
| aead (trait) | 0.6.1 | 156,403,349 | 33,855,299 | RustCrypto/traits | — |

两者**同仓同 API**(都 impl `aead::Aead` trait),换算法只需换类型别名。差异:
- **AES-GCM**:x86/x86_64 上走 AES-NI + CLMUL 硬件指令(本项目主目标平台);部署面最广(TLS / WiFi / IPsec 标准)。
- **ChaCha20Poly1305**:纯软件常时间实现更稳(无硬件加速依赖);移动端 / 非 x86 友好;nonce 长度同 96-bit,另有 `XChaCha20Poly1305` 24-byte nonce 变体(容忍 nonce 生成更宽松)。

本项目运行环境是桌面 PC(WSL2/RDP 双屏),AES-NI 几乎必有 → **AES-GCM** 性能/审计/部署面综合最优。

#### 依赖说明

- `aead` crate (`Aead`/`KeyInit`/`AeadCore`/`OsRng` 等 trait)被 `aes-gcm` **re-export**(`use aes_gcm::aead::{...}`),不需要单独列 `aead` 依赖。
- `rand_core` 同样由 `aead` re-export(`pub use common::rand_core`)→ `OsRng` 可直接用。
- nonce 生成用 `Aes256Gcm::generate_nonce(&mut OsRng)`,**不需要** `rand`/`getrandom` 直接依赖(`OsRng` 底层走 `getrandom` crate,Linux 上是 `getrandom(2)` syscall,WSL 原生支持,见 §3)。

#### 最小可用代码示例 (encrypt + decrypt 一个 String)

来自 `docs.rs/aes-gcm` 官方 Usage 示例,改成 String 往返:

```rust
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};

// master_key: 32 字节,由 HKDF 从 machine-id 派生(见 §3)
fn encrypt_secret(master_key: &[u8; 32], plaintext: &str) -> Result<Vec<u8>, aes_gcm::Error> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 96-bit,每条密文一个,随机即可
    let ct = cipher.encrypt(&nonce, plaintext.as_bytes())?;
    // ct 已包含 16-byte Poly1305 tag(append 在密文末尾,由 aead crate 自动处理)
    // 打包 nonce + ct(见 §2)
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn decrypt_secret(master_key: &[u8; 32], mut blob: &[u8]) -> Result<String, aes_gcm::Error> {
    let (nonce_bytes, ct) = blob.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let nonce = Nonce::from_slice(nonce_bytes); // 12 bytes
    let pt = cipher.decrypt(nonce, ct)?;
    String::from_utf8(pt).map_err(|_| aes_gcm::Error) // aes_gcm::Error 是 unit struct
}
```

来源:
- [docs.rs/aes-gcm Usage](https://docs.rs/aes-gcm/latest/aes_gcm/) — `Aes256Gcm::generate_nonce(&mut OsRng)` + `cipher.encrypt(&nonce, plaintext)`
- [docs.rs/aead traits](https://docs.rs/aead/latest/aead/) — `Aead` / `KeyInit` / `AeadCore` / `rand_core` re-export
- [RustCrypto/AEADs README](https://github.com/RustCrypto/AEADs/blob/master/README.md) — MSRV 1.85,AEADs 全家族表格

---

### 2. 密文存储格式:nonce + ciphertext + tag 打包

#### AEAD 密文构成

`Aes256Gcm::encrypt(&nonce, msg)` 返回的 `Vec<u8>` **已自带 16-byte Poly1305 认证 tag**(append 在密文尾部,由 `aead` crate 内部处理,调用方无需手动拼)。所以一条密文实际是:

```
ciphertext_bytes = plaintext_len bytes XOR keystream
tag              = 16 bytes (Poly1305 over AAD + nonce + ciphertext)
encrypt() 返回   = ciphertext_bytes || tag   (连续,长度 = plaintext_len + 16)
```

#### 推荐打包格式 (DB 单列)

```
blob = nonce(12B) || ciphertext || tag(16B)
     = nonce(12B) || encrypt()返回值
```

- **固定头部 12B nonce** → 解密时 `split_at(12)` 直接拿。
- 不需要单独存 tag(已在 encrypt 返回里)。
- 不需要版本/魔数字段;但**强烈建议**加 1-byte 版本前缀(`0x01 || nonce || ct||tag`)以便未来换算法(如换 ChaCha20Poly1305 / 换 KDF)时老密文能识别。代价极小,收益大。

#### DB TEXT 列怎么存 (base64 vs hex)

二进制 `blob` 不能直接进 SQLite TEXT(不可打印/编码问题),两种选择:

| 方式 | 长度膨胀 | 大小写敏感 | Rust crate | 推荐 |
|---|---|---|---|---|
| **base64** | ×1.33 | 否 | `base64` 0.22(已有大量依赖,本项目 `tiktoken-rs` 等间接用) | ✅ |
| **hex** | ×2.0 | 否 | `hex` crate 或 `fmt` 手写 | ❌ 太长 |

**推荐 base64 standard alphabet**(或 url-safe,**必须选一个并固定**)。典型 api_key ~40-60B 明文 → nonce 12 + ct ~60 + tag 16 = ~88B 二进制 → base64 ~120 字符,完全可接受。

`base64` crate 用法:
```rust
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
let stored: String = B64.encode(&blob);          // 写 DB
let blob: Vec<u8> = B64.decode(&stored)?;        // 读 DB
```

#### Schema 设计:`api_key` 列存密文 vs 加 `key_encrypted` 新列

**两个方案**:

**方案 A(就地加密,in-place)**:复用现有 `providers.api_key TEXT` 列,值改成 base64 密文。
- 优点:不动 schema,wire (`ProviderRow.apiKey`) 字段名不变(只是语义从明文→密文/哨兵)。
- 缺点:**无法区分"还没迁移的明文"vs"已迁移的密文"**(除非加哨兵前缀或专门 flag);`list_providers` IPC 若直接回传密文,前端要理解"这是密文不是 key"。
- 空值语义:`api_key = ''`(现有 DEFAULT)继续表示"无 key";加密后空串仍存空串(不加密空值)。

**方案 B(加新列 + 迁移后抹旧列)**:加 `providers.api_key_enc TEXT`(存密文),保留旧 `api_key` 列迁移后 `UPDATE ... SET api_key = ''`。
- 优点:**明文/密文可共存**,迁移可分步幂等,回滚安全;密文列空 = 未迁移,旧列空+新列有 = 已迁移。
- 缺点:多一列,wire 要决定回传哪个;迁移完成后理论上可 `ALTER`(但 SQLite 不支持 DROP COLUMN < 3.35,且本项目用 PRAGMA probe 模式,留空列无成本)。

**推荐方案 B**(符合现有 migration 体系 `add_*_column_if_missing` + 幂等 UPDATE 模式,见 §4)。Wire 形态:`list_providers` IPC 不再回传任何 key(明文或密文都不回传,见 prd Open Q #2),前端 Settings 改"留空=保持/输入新值=覆盖"。

---

### 3. master key 来源方案对比 + 安全边界 (核心)

#### (a) 编译期固定 key —— 不推荐

把一个 32 字节 key 编进二进制(`const MASTER_KEY: [u8; 32] = [...];`)或 hash 一段常量字符串。
- **多弱**:任何拿到二进制的人(`strings` / `objdump` / 反编译)几秒就能拿到 key。`cargo build --release` + `strip = true`(本项目 Cargo.toml 已开)**剥不掉**嵌入的常量数据。一旦 key 泄露,**全用户、全安装、全版本**的 DB 都可被解密——比单机明文还差(明文至少要拿到那台机器的 DB)。
- **何时可接受**:加密只是为了满足"DB 字段不是明文可读"的形式合规,且明确不防任何人。本项目威胁模型要防"DB 文件泄露",**此方案不达标**。

#### (b) 机器绑定派生 —— 推荐

读取本机唯一标识 → HKDF/argon2 派生 32B master key → 常驻内存,启动时算一次。

**能否防住"DB 被拷走到另一台机器"?** ✅ **能**。攻击者只有 DB 文件 → 没有 victim 机器的 machine-id → 派生出的 master key 不同 → 解密失败(`aes_gcm::Error`)。这正是本威胁模型要的。

**防不住什么**(明确 Out of Scope):
- 攻击者连 machine-id 一起拿走(整盘克隆 / `/etc/machine-id` 被读)。
- 攻击者拿到运行中进程的内存(`gcore` / debugger 抓 master key)。

**machine-id 的安全性质**(来自 machine-uid README 原文):"This ID uniquely identifies the host. It should be considered 'confidential', and must not be exposed in untrusted environments. And do note that the machine id can be re-generated by root." → 即:它本身不是密钥,是**绑定因子**。在"防 DB 泄露"模型下作为 HKDF 的 IKM 是足够的;在更高威胁模型下应改用 OS keystore 存真正的随机 master key。

**跨平台取 machine-id**:

| crate | 版本 | 下载量 | 平台覆盖 | 备注 |
|---|---|---|---|---|
| **`machine-uid`** | 0.6.0 | 2,884,838 | Linux/BSD/macOS/Windows/illumos | ✅ 推荐,覆盖全,API 极简 `machine_uid::get() -> Result<String>` |
| `machine-uuid` | 0.1.0 | 5,961 | (少) | 下载量太小,不推荐 |
| 手写 | — | — | — | 平台分支多(winreg/gethostuuid),没必要重复造 |

`machine-uid` 各平台数据源(README 原文):
- **Linux / systemd**:`/var/lib/dbus/machine-id` 或 `/etc/machine-id`(32 hex char lowercase,16 字节值)—— **本项目 WSL2 实测两者都存在且一致**。
- **macOS**:`gethostuuid(3)`(= `ioreg -rd1 -c IOPlatformExpertDevice` 的 `IOPlatformUUID`)。
- **Windows**:`HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`(注册表)。
- BSD:`/etc/hostid` 或 `kenv smbios.system.uuid`。
- illumos:`gethostid(3C)`。

**派生方式选择:HKDF vs argon2**

- **HKDF**(推荐,本场景):IKM(machine-id)是**高熵但非均匀**的 32 hex 字符串 → HKDF extract 把它压成均匀 PRK → expand 出 32B key。`Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut okm)`。**快**(微秒级),启动一次性开销可忽略。machine-id 不是用户密码,**不需要** argon2 的慢哈希抗暴力(machine-id 攻击者要么知道要么不知道,没有"暴力"空间)。
- **argon2**(本场景过重):用于从**低熵+人脑记忆**的密码派生(抗 GPU 暴力)。machine-id 是机器自动的、32 hex 的高熵值,用 argon2 是杀鸡用牛刀,且启动多几十~几百 ms。

HKDF 用法(来自 docs.rs/hkdf 官方示例):
```rust
use hkdf::Hkdf;
use sha2::Sha256;

fn derive_master_key(machine_id: &str) -> [u8; 32] {
    let salt = b"everlasting::provider-key-v1"; // 固定,绑应用+版本,换算法时 bump
    let info = b"aes-256-gcm master key";        // context string,绑用途
    let hk = Hkdf::<Sha256>::new(Some(salt), machine_id.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm).expect("32 bytes is valid for Sha256");
    okm
}
```

#### (c) OS keystore 存 master key —— 本项目回避

生成一个真随机 32B master key 存进 keyring(macOS Keychain / Windows Credential Manager / Linux Secret Service)。比 (b) 强(key 是真随机,不依赖 machine-id)。
- **问题**:**又依赖 keyring crate**,而 keyring 在 WSL2/Linux 无 secret service 时不可用(见 `research/keyring-wsl-availability.md`)——这正是本 fallback 方案要回避的依赖。若未来 keyring 可用性解决,可作为 (b) 的升级(把 HKDF 派生换成"读 keyring 里的随机 key")。

#### (d) 用户密码派生 —— 本项目 UX 不可接受

每次启动要用户输密码 → argon2 派生 → master key。最强(防"DB+machine-id 同时泄露"),但:① Tauri 桌面 app 每次启动弹密码框,UX 灾难;② 密码忘了 = 所有 provider key 永久不可恢复。**不符合** prd 的"运行时仍能发请求"低摩擦要求。Out。

#### 结论:方案 (b) 在"只防 DB 文件泄露"威胁模型下足够 ✅

(b) 恰好覆盖 prd 的威胁模型边界:防 DB 泄露 ✅,不防 root/进程内存 ✅(符合 Out of Scope),不引入 keyring 依赖 ✅(WSL 友好),零 UX 摩擦 ✅。

---

### 4. 迁移:现有明文 → 加密列

#### 项目现有 migration 体系(已读 `app/src-tauri/src/db/migrations.rs` 确认)

- `run_migrations(pool)` 幂等,每次启动调用。
- 模式 1:**加列幂等** = `add_<table>_column_if_missing(pool, "col", "DECL")`,内部 `SELECT COUNT(*) FROM pragma_table_info('<table>') WHERE name = ?` 探测,不存在才 `ALTER TABLE ... ADD COLUMN`。现有 4 个 helper(`add_session/project/messages/subagent_runs_column_if_missing`),`providers` 表**还没有**对应 helper(需新增 `add_provider_column_if_missing`)。
- 模式 2:**幂等 UPDATE 回填** = `UPDATE ... SET col = ... WHERE col IS NULL OR col = ''`(见 migrations.rs:636-645 的 `current_cwd` 回填,以及 :396 `SET mode='edit' WHERE mode IS NULL`)。

#### 推荐迁移步骤(方案 B,分步幂等)

```rust
// migrations.rs 新增,接在现有 v6 providers 表迁移之后

// Step 1: 加密文列(幂等,复用 PRAGMA probe 模式)
add_provider_column_if_missing(pool, "api_key_enc", "TEXT NOT NULL DEFAULT ''").await?;

// Step 2: 加迁移完成标志列(幂等)—— 防重复迁移的哨兵
add_provider_column_if_missing(pool, "key_migrated_at", "TEXT").await?;

// Step 3: 一次性回填——把明文加密写进 api_key_enc,清空 api_key
//   WHERE 条件保证幂等:只处理"还有明文且还没迁移"的行
//   注意:这一步必须在 Rust 代码里做(要用 HKDF + AES-GCM),
//         不能纯 SQL。见下方 Rust 代码骨架。
let rows = sqlx::query(
    "SELECT id, api_key FROM providers
     WHERE api_key <> '' AND key_migrated_at IS NULL"
).fetch_all(pool).await?;
let mk = derive_master_key(&machine_uid::get()?);
for row in rows {
    let id: String = row.try_get("id")?;
    let plain: String = row.try_get("api_key")?;
    let enc = B64.encode(&encrypt_secret(&mk, &plain)?);
    sqlx::query(
        "UPDATE providers
            SET api_key_enc = ?, api_key = '', key_migrated_at = ?
          WHERE id = ?"
    ).bind(&enc).bind(Utc::now().to_rfc3339()).bind(&id)
     .execute(pool).await?;
}
```

**幂等性保证**:
- 重复启动 → Step 1/2 的 `add_*_column_if_missing` 是 no-op。
- Step 3 的 `WHERE api_key <> '' AND key_migrated_at IS NULL` → 已迁移行(`api_key=''`)被跳过,不重复加密。
- 中途失败崩溃 → 部分行已迁移(`api_key=''`+`key_migrated_at` 有值),未迁移行下次启动继续;无重复、无数据丢失。
- **原子性**:SQLite 每个 UPDATE 自身是事务;若要"加密+抹旧"原子,可包 `BEGIN/COMMIT`(sqlx `pool.begin().await?`),但行级幂等已足够,不强求事务。

#### 运行时解密落点(prd 已锁定)

- `app/src-tauri/src/agent/provider.rs:107 resolve_chat_provider` → 读 `api_key_enc` → `decrypt_secret(&mk, &enc)` → 明文交给 provider factory。
- `app/src-tauri/src/agent/chat.rs:342` 同 pre-flight(走 catalog fast-path)。
- `app/src-tauri/src/commands/providers.rs:367/409 test_model` → 同样现解密。
- 这三处是 prd 确认的"唯一明文消费点",解密 helper 在此处注入即可,不动 LLM 请求链路。

#### `list_providers` IPC 改动(配合 prd Open Q #2)

不再回传 `apiKey` 明文。`ProviderRow` 的 `apiKey` 字段要么:
- 移除(前端 type 改);或
- 改名 `apiKeyPresent: bool` / 回传是否设置了 key(不回传值)。
前端 Settings 编辑改"留空=保持不变,输入新值=覆盖"(业界 secret 输入标准 UX)—— 这是 prd Open Q #2 的决策项,非本 research 范围,但加密方案与之正交兼容。

---

## 推荐方案完整骨架

### Cargo.toml 增量(5 个直接依赖,`aead`/`rand_core` 由 aes-gcm re-export 不单列)

```toml
# app/src-tauri/Cargo.toml [dependencies] 追加

# P1 API key 加密 (RULE-D-001): AES-256-GCM + HKDF(machine-id)
# - aes-gcm: RustCrypto AEAD,aead/rand_core trait re-export,OsRng 走 getrandom(WSL 可用)
# - hkdf + sha2: 从 machine-id 派生 32B master key(Hkdf<Sha256>)
# - machine-uid: 跨平台取 /etc/machine-id / IOPlatformUUID / MachineGuid
# - base64: blob(nonce+ct+tag) ↔ DB TEXT 编码
aes-gcm = "0.10"
hkdf = "0.12"
sha2 = "0.10"
machine-uid = "0.6"
base64 = "0.22"
```

> 版本说明:`aes-gcm` 0.10.3 是稳定版(rc.4 不是 release);`hkdf` 0.12 / `sha2` 0.10 是匹配 `aead` 0.5 trait 的稳定线。若选 `aes-gcm` 0.11-rc 需同步 `aead` 0.6 / `hkdf` 0.13,会拖入更多 rc,不推荐生产用。

### 加密模块签名 (建议新建 `app/src-tauri/src/crypto.rs`)

```rust
//! 应用层对称加密: AES-256-GCM + HKDF(machine-id) 派生 master key.
//! 威胁模型: 防 DB 文件泄露,不防本机 root / 进程内存.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hkdf::Hkdf;
use sha2::Sha256;

const VERSION: u8 = 0x01; // 打包格式版本,换算法/KDF 时 bump
const SALT: &[u8] = b"everlasting::provider-key::v1";
const INFO: &[u8] = b"aes-256-gcm master key";

/// 从 machine-id 派生 32B master key. 启动时调用一次,缓存进 AppState.
pub fn derive_master_key() -> anyhow::Result<[u8; 32]> {
    let mid = machine_uid::get().map_err(|e| anyhow::anyhow!("machine_uid: {e}"))?;
    let hk = Hkdf::<Sha256>::new(Some(SALT), mid.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(INFO, &mut okm).map_err(|e| anyhow::anyhow!("hkdf expand: {e}"))?;
    Ok(okm)
}

/// 加密: blob = VERSION(1) || nonce(12) || ct || tag(16), base64 编码.
pub fn encrypt(master_key: &[u8; 32], plaintext: &str) -> anyhow::Result<String> {
    if plaintext.is_empty() { return Ok(String::new()); } // 空 key 不加密,存空串
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher.encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("aes-gcm encrypt: {e}"))?;
    let mut blob = Vec::with_capacity(1 + 12 + ct.len());
    blob.push(VERSION);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    Ok(B64.encode(&blob))
}

/// 解密: 入参是 encrypt() 的输出(空串原样返回空).
pub fn decrypt(master_key: &[u8; 32], stored: &str) -> anyhow::Result<String> {
    if stored.is_empty() { return Ok(String::new()); }
    let blob = B64.decode(stored).map_err(|e| anyhow::anyhow!("base64: {e}"))?;
    let (&ver, rest) = blob.split_first().ok_or_else(|| anyhow::anyhow!("empty blob"))?;
    anyhow::ensure!(ver == VERSION, "unknown ciphertext version {ver}");
    let (nonce_bytes, ct) = rest.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key));
    let pt = cipher.decrypt(Nonce::from_slice(nonce_bytes), ct)
        .map_err(|e| anyhow::anyhow!("aes-gcm decrypt: {e}"))?;
    String::from_utf8(pt).map_err(|e| anyhow::anyhow!("utf8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let mk = derive_master_key().unwrap();
        let ct = encrypt(&mk, "sk-test-12345").unwrap();
        assert_ne!(ct, "sk-test-12345");
        assert_eq!(decrypt(&mk, &ct).unwrap(), "sk-test-12345");
    }
    #[test]
    fn empty_stays_empty() {
        let mk = derive_master_key().unwrap();
        assert_eq!(encrypt(&mk, "").unwrap(), "");
        assert_eq!(decrypt(&mk, "").unwrap(), "");
    }
    #[test]
    fn tamper_fails() {
        let mk = derive_master_key().unwrap();
        let mut ct = encrypt(&mk, "secret").unwrap();
        let mut bytes = B64.decode(&ct).unwrap();
        bytes[13] ^= 0xff; // 翻转一个 ciphertext 字节
        ct = B64.encode(&bytes);
        assert!(decrypt(&mk, &ct).is_err()); // 认证失败
    }
}
```

### DB 列设计

```
providers 表新增两列(幂等 migration):
  api_key_enc    TEXT NOT NULL DEFAULT ''   -- base64(VERSION||nonce||ct||tag),空串=无 key
  key_migrated_at TEXT                       -- RFC3339 时间戳,迁移完成哨兵
旧 api_key 列保留(迁移后 UPDATE 为 ''),SQLite 无 DROP COLUMN < 3.35,留空列无成本。
```

### 迁移步骤(幂等,见 §4 代码)

1. `add_provider_column_if_missing("api_key_enc", ...)` (新增 helper,复用 PRAGMA probe 模式)
2. `add_provider_column_if_missing("key_migrated_at", ...)`
3. Rust 循环:`SELECT id, api_key WHERE api_key<>'' AND key_migrated_at IS NULL` → `encrypt` → `UPDATE ... SET api_key_enc=?, api_key='', key_migrated_at=?`

### 运行时接入

- `AppState` 启动时算一次 `derive_master_key()`,缓存。
- `resolve_chat_provider` / `test_model` 读 `api_key_enc` → `decrypt(&mk, &enc)` → 明文发请求。
- `list_providers` IPC 不回传 key(配合 prd Open Q #2 secret 输入 UX)。

---

## 外部参考

- [docs.rs/aes-gcm](https://docs.rs/aes-gcm/latest/aes_gcm/) — 官方 Usage 示例(`Aes256Gcm::generate_nonce(&mut OsRng)` + `encrypt/decrypt`)
- [docs.rs/aead](https://docs.rs/aead/latest/aead/) — `Aead`/`KeyInit`/`AeadCore`/`rand_core` trait(`OsRng` 来源)
- [docs.rs/hkdf](https://docs.rs/hkdf/latest/hkdf/) — `Hkdf::<Sha256>::new(salt, ikm).expand(info, okm)` 用法
- [docs.rs/chacha20poly1305](https://docs.rs/chacha20poly1305/latest/chacha20poly1305/) — 备选 AEAD,同 trait API
- [RustCrypto/AEADs README](https://github.com/RustCrypto/AEADs) — MSRV 1.85,全家族对比表,aes-gcm 与 chacha20poly1305 同仓
- [aes-gcm NCC Group 安全审计](https://web.archive.org/web/20240108154854/https://research.nccgroup.com/wp-content/uploads/2020/02/NCC_Group_MobileCoin_RustCrypto_AESGCM_ChaCha20Poly1305_Implementation_Review_2020-02-12_v1.0.pdf) — 无重大发现
- [machine-uid README](https://github.com/Hanaasagi/machine-uid) — 各平台 machine-id 来源(Linux `/etc/machine-id`,macOS `gethostuuid`,Win `MachineGuid`),版本 0.6.0
- [docs.rs/getrandom](https://docs.rs/getrandom/latest/getrandom/) — Linux 走 `getrandom(2)` syscall(WSL 原生支持),即 `OsRng` 底层

---

## Caveats / Not Found

- **machine-id 跨 WSL 发行版重置**:`wsl --unregister` / 重装发行版会重新生成 `/etc/machine-id` → 旧密文不可解密。这是方案 (b) 的固有性质(机器绑定),非 bug。若担心,文档提示用户"重装 WSL 前需在 Settings 重新粘贴 provider key"。**未在 research 中验证** `wsl --update` / Windows 大版本升级是否会重置(需实测,但不阻塞方案选定)。
- **keyring 路线对比**:本 research 只覆盖 fallback 主方案(应用层加密)。keyring 在 WSL 可用性、业界同类工具(Cursor / VS Code / Zed)key 存储实践在 prd 列的另两个 research 文件中(`keyring-wsl-availability.md` / `industry-api-key-storage.md`),本文件不重复。
- **`aes-gcm` 0.11-rc vs 0.10.3**:0.11 是 rc(拖入 `aead` 0.6 / `hkdf` 0.13 的 rc 链),生产建议锁 0.10.3 + 匹配的 `hkdf` 0.12 / `sha2` 0.10。骨架 Cargo.toml 已按稳定线给版本。
- **AAD (Associated Data)**:本骨架未用 AAD(关联数据)。若想绑定"这个密文是给 provider X 用的",可把 `provider.id` 作 AAD 传入 `encrypt(&nonce, Payload { msg, aad: provider_id })`,这样把 A 行的密文复制到 B 行解密会失败。prd 未要求,**骨架未加**,列为可选增强。
- **未查**:本机 `keyring` crate 实际在 WSL2 能否工作(那是另一份 research);本方案选应用层加密正是**回避**该不确定性,故不阻塞。
