# Research: keyring crate 在 WSL2 + Ubuntu 下的可用性

- **Query**: Rust `keyring` crate(及替代后端)在 WSL2 + Ubuntu 下能否用于存 API key;无 secret service daemon 时的行为;keyutils 文件 fallback;跨平台一致性
- **Scope**: external(官方 docs.rs / GitHub issue / Cargo.toml / wiki)+ internal(本地环境实测)
- **Date**: 2026-06-24
- **方法说明**: 无 WebSearch / MCP exa 工具可用,改用 `curl` 直接抓 `docs.rs` / `api.github.com` / `raw.githubusercontent.com` 原始文档与 issue 全文。所有引用均带可核验 URL。

---

## 结论先行(一句话)

**WSL2 默认环境下 `keyring` crate 开箱不可用** —— v1 默认 Linux 后端 `zbus-secret-service-keyring-store` 需要 `org.freedesktop.secrets` provider(gnome-keyring / kwallet),而 WSL2 默认只装了 dbus 守护进程、**没装 gnome-keyring**(本机实测确认),且 secret-service store 在 WSL 下还有"无 default collection"的二次坑。**对本项目(WSL-first 个人 Tauri 桌面 app):不推荐用 OS secret store 作为主方案,推荐"应用层对称加密(AES-GCM)+ master key"为主**,keyring 仅在 macOS/Windows 原生后端 + Linux headless fallback 时按需启用,且必须设计成"失败可降级"。

---

## 关键架构背景(v3/v4 起 crate 拆分)

`keyring` crate(当前 **4.1.2**,仓库已迁至 `open-source-cooperative/keyring-rs`,最近 push `2026-06-22`,活跃维护,739 stars)已从"单 crate 多平台"重构为 **facade + 可插拔 store crate** 架构:

| store crate | 后端 | keyring v1 默认? | keyring `cli` feature? |
|---|---|---|---|
| `apple-native-keyring-store` | macOS Keychain Services | 是(macOS) | 是 |
| `windows-native-keyring-store` | Windows Credential Manager | 是(Windows) | 是 |
| `zbus-secret-service-keyring-store` | Secret Service via zbus(Rust 纯 D-Bus) | **是(Linux)** | 是 |
| `dbus-secret-service-keyring-store` | Secret Service via dbus-secret-service(C 库) | 否 | 是 |
| `linux-keyutils-keyring-store` | kernel keyutils(keyrings(7)) | **否** | 是 |
| `db-keystore` | 应用自带加密 SQLite | 否 | 是 |

来源:`keyring` facade `Cargo.toml`(`https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/main/Cargo.toml`)的 `[features]` 段:
```toml
default = ["v1"]
v1 = ["apple-native-keyring-store/keychain", "windows-native-keyring-store", "zbus-secret-service-keyring-store"]
cli = [..., "dbus-secret-service-keyring-store", "linux-keyutils-keyring-store", "db-keystore"]
```

> **这意味着**:如果只开 `default-features`(=v1),Linux 上一条路走到黑就是 zbus secret-service,没有 keyutils 文件 fallback。要用 keyutils 必须显式开 `cli` 或直接依赖 `keyring-core` + 选定 store。

---

## 各必查项发现

### 1. Linux 后端机制 + WSL 默认是否装 daemon

**机制**(v1 module docs `https://docs.rs/keyring/latest/keyring/v1/`):
> "On \*nix operating systems, the secure credential store is the Secret Service."

Secret Service API(Freedesktop 规范)通过 D-Bus 暴露 `org.freedesktop.secrets`,需要有个 provider daemon 在 session bus 上应答。常见 provider:`gnome-keyring-daemon`(GNOME)、`kwalletd`(KDE)、`keepassxc`(可选)。

**WSL2 默认装了吗 —— 本机实测(2026-06-24,这台开发机):**
```
Linux version 5.15.153.1-microsoft-standard-WSL2
WSLg 在线: /mnt/wslg 存在, WAYLAND_DISPLAY=wayland-0, DISPLAY=:0, weston.log 有内容
D-Bus 已装且 session bus 在跑: DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
  dbus-daemon     /usr/bin/dbus-daemon       (installed)
  dbus-run-session /usr/bin/dbus-run-session (installed)
  gnome-keyring-daemon  → NOT FOUND          ← 关键:没有 secret service provider
  seahorse              → NOT FOUND
```
**结论:WSL2 + Ubuntu 默认有 D-Bus 总线、但无 secret service provider。总线活着没人应答 `org.freedesktop.secrets`。** WSLg 改变了 Wayland/X11 可用性,但**不自动装 gnome-keyring**。

### 2. 无 secret service daemon 时的行为(是否 panic / 可 fallback)

**`keyring::v1::Error` 枚举**(`https://docs.rs/keyring/latest/keyring/v1/enum.Error.html`,`#[non_exhaustive]`):
```rust
pub enum Error {
    PlatformFailure(Box<dyn Error + Send + Sync>),
    NoStorageAccess(Box<dyn Error + Send + Sync>),  // store 锁住 / 不可访问
    NoEntry,
    BadEncoding(Vec<u8>),
    BadDataFormat(...),
    BadStoreFormat(String),
    TooLong(String, u32),
    Invalid(String, String),
    Ambiguous(Vec<Entry>),
    NoDefaultStore,                                  // 无默认 store
    NotSupportedByStore(String),
}
```
`Error` 实现 `std::error::Error` + `Debug`/`Display`,**全部为可返回的 `Result` 值,不 panic**。

**`Entry::new` 的初始化逻辑**(`src/v1.rs:88-113`):
```rust
pub fn new(service: &str, username: &str) -> Result<Self> {
    SET_CREDENTIAL_STORE.call_once(set_credential_store);   // 一次性
    let inner = keyring_core::Entry::new(service, username)?;
    Ok(Self { inner })
}
fn set_credential_store() {
    #[cfg(all(unix, not(...)))] {
        if let Ok(store) = zbus_secret_service_keyring_store::Store::new() {  // 尝试连 D-Bus
            keyring_core::set_default_store(store);
        }
        // 失败则静默不设默认 store —— 后续 Entry::new 返回 NoDefaultStore
    }
}
```
**关键:`Store::new()` 失败不 panic,只是 `if let Ok(...)` 跳过;`Entry::new` 随后返回 `NoDefaultStore`(或后续 `set_password` 返回 `NoStorageAccess`)。应用完全可以 `match` 捕获并走 fallback。**

**历史 panic 案例(早期版本,现已修复)**:issue #15(2018)`Panic on ubuntu-server / wsl`:
```
thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value:
SecretServiceError(Dbus(D-Bus error: Unable to autolaunch a dbus-daemon without a $DISPLAY for X11
(org.freedesktop.DBus.Error.NotSupported)))'
```
这是**用户代码 `.unwrap()` 导致的 panic,不是 crate panic**;现代版本返回 `Error` 而非 panic。issue #83(2022)用户报告 Linux 上返回 `PlatformFailure: I/O error: No such file or directory` —— 也是 error 不是 panic。

> ✅ **可 fallback**:`match` `NoStorageAccess` / `NoDefaultStore` / `PlatformFailure` 三个 variant 即可。

### 3. 替代后端:`linux-keyutils-keyring-store`(kernel keyring)

官方文档(`https://docs.rs/linux-keyutils-keyring-store/latest/linux_keyutils_keyring_store/`)原话:
> "If you are trying to use the keyring crate on a headless linux box, or one that doesn't come with gnome-keyring, **it's strongly recommended that you use this credential store, because (as part of the kernel) it's always available on Linux.**"

设置默认 store(只需一行):
```rust
keyring_core::set_default_store(linux_keyutils_keyring_store::Store::new().unwrap())
```

**致命限制 —— 不持久化跨重启**(同一文档 `Persistence` 小节):
> "The key management facility provided by the kernel is **completely in-memory and will not persist across reboots**. Consider the keyring a **secure cache** and plan for your application to handle cases where the entry is no longer available in-memory. ... Potential options to re-load the credential into memory are: Re-prompt the user (most common/effective for CLI applications); Create a PAM module or use pam_exec; if running as systemd service use systemd-ask-password."

**是否需要 root**:kernel keyutils 有 session keyring(每登录会话)/ user keyring(每用户)/ user-session keyring 等层级,**普通用户即可读写自己的 keyring**,不需 root。但"重启即丢"意味着:每次 WSL 重启(WSL2 默认 `wsl --shutdown` 或重启 Windows 后)key 全没了,app 必须重新拉用户输 key 或从别处重载 —— **对 API key 这种长期凭证场景不可接受**(用户不会每次开机重输 N 个 provider key)。

**其他坑**:issue #266(2025-08)`set_password returning NoEntry but Entry exists` —— 在 gnome 47 + Linux 6.14 上观察到 keyutils 路径下的写入竞态/丢失,说明 keyutils 后端也有稳定性疑虑。

### 4. WSL2 装 gnome-keyring + `dbus-run-session` 可行性

**官方文档(`dbus-secret-service-keyring-store`)有专门一节 "Usage on Windows Subsystem for Linux"**(`https://docs.rs/dbus-secret-service-keyring-store/`,文档侧栏导航可见):
> "**Usage on Windows Subsystem for Linux** — As noted in this issue on GitHub, **there is no 'default' collection defined under WSL.** So this crate **will not work on WSL unless you specify a non-default target modifier on every specifier.**"

**这意味着即使装了 gnome-keyring,在 WSL 下仍要在每个 `Entry` 上指定 target modifier(`Entry::new_with_target` 或 v1 等价 API)才能用**,默认 default-collection 路径直接失败。来源 issue:#133(`Automatically fallback if a default Linux keyring is not available?`,2023)。

维护者 brotskydotcom 在 #133 的明确建议(两条):
- (a) WSL 下 default collection 通常由 GNOME 经 PAM 创建,而 **WSL(1 和 2)即便跑 GUI 也不用 PAM 登录**,所以没有 default collection;需要"运行一些 secret-service 代码在 WSL 上创建一个 default collection","每台机器只需做一次,但幂等可每次启动做"。
- (b) **"my suggestion would be that you use the Windows build of keyring rather than the Linux one"** —— 即建议 WSL 用户直接跑 Windows 二进制走 Windows Credential Manager。**对本项目不可行**:Tauri 在 WSL 下是 Linux 构建,无法切 Windows backend。

**Headless Linux 启动 gnome-keyring 的官方 workaround**(同文档 "Headless usage" 小节):
```bash
function unlock-keyring () {
  read -rsp "Password: " pass
  echo -n "$pass" | gnome-keyring-daemon --unlock
  unset pass
}
```
参考项目 CI(`open-source-cooperative/keyring-rs` 的 GitHub Actions workflow)和 Python keyring 文档 "Using Keyring on headless Linux systems"。

**可行性评估(个人 dev 机器):**
- 装 `gnome-keyring`:`sudo apt install gnome-keyring`(拉 GNOME 一堆依赖,~几十 MB,个人 dev 机器可接受)
- 启动:`dbus-run-session` 包裹 + `gnome-keyring-daemon --unlock`(需要给个解锁密码,要么空密码要么硬编码)—— **每次 WSL 会话启动都要跑一遍**,或写进 `~/.bashrc`/systemd user unit
- 即便如此仍要处理"WSL 无 default collection"二次坑(每次启动创建/解锁 default collection,或代码里指定 non-default target)
- **对普通用户太重,对个人 dev 可忍但不优雅**。且 WSLg 下 gnome-keyring 弹窗 GUI 解锁对话框会走 X11/Wayland 转发(connor4312 在 #133 提到 "I see the ubuntu keyring password prompt"),体验割裂。

### 5. 跨平台一致性(macOS Keychain / Windows Credential Vault)

v1 module docs 明确:
> "On macOS, the secure credential store is Keychain Services. On Windows, the secure credential store is the Windows Credential Manager."

这两条路径**业界成熟、开箱可用、无 WSL 类坑**:
- macOS:用户首次访问会弹系统授权对话框(可"始终允许"消除),无 daemon 依赖。
- Windows:Credential Manager 是系统服务,无 daemon 依赖。

**Windows 已知坑**(issue #270,2025-08):`set_password` 对**长 secret(~360 chars,如 OAuth refresh token)在 Windows 上报 `PlatformFailure: Windows error code 8`**(ERROR_INSUFFICIENT_BUFFER / NOT_ENOUGH_MEMORY)。Credential Manager 单条 secret 上限约 2560 字节(实际 API `CredWrite` 的 `CredentialBlob` 上限 512 字节明文 = ~511 char,CredProtect 后更少)。**API key 一般 < 256 char 不会撞**,但若存大 token 需注意。注:reporter 自述是 Tauri 桌面 app。

macOS 无类似长度问题(Keychain item 可存 MB 级)。

---

## 对本项目的明确推荐

**项目特征回顾**:WSL-first(主开发+运行环境)、个人 Tauri 桌面 app、威胁模型是"DB 文件整体泄露 ≠ key 泄露"(非防 root/进程内存)、需可逆解密。

**推荐:应用层对称加密(AES-GCM)+ master key 为主;keyring 仅作"master key 寄存"的可选优化,且必须可降级。**

理由矩阵:

| 方案 | WSL2 可用 | 持久化 | 跨平台一致 | 实现成本 | 对本项目结论 |
|---|---|---|---|---|---|
| **v1 默认(zbus secret-service)** | ❌ 默认无 gnome-keyring;装了还有"无 default collection"坑 | ✅ | Linux 与 macOS/Win 不一致 | 低(开箱) | **不可行(WSL 开箱即坏)** |
| **keyutils(kernel keyring)** | ✅ 总可用 | ❌ **重启即丢** | Linux 专属 | 低 | **不可行(API key 长期凭证不能每次重输)** |
| **WSL 装 gnome-keyring + dbus-run-session** | ⚠️ 可但需每次会话启动 + 处理 default collection 坑 | ✅ | 仅 Linux;macOS/Win 走各自原生 | 中-高(需运维脚本) | **个人 dev 可忍,但不优雅,且与"WSL-first 开箱即用"理念冲突** |
| **应用层 AES-GCM + master key(机器绑定)** | ✅ | ✅ | ✅ 一套代码全平台 | 中(需加密库 + master key 来源 + 迁移) | **推荐(主方案)** |
| **混合:keyring 存 master key + AES-GCM 加密 provider key** | ⚠️ WSL 仍走 keyring 坑 | ✅ | ✅ | 高 | **可选优化,非 MVP** |

**master key 来源**(若走加密路线,呼应 PRD Open Question 4):本机实测 `/etc/machine-id` 存在且稳定(`b320623d...`,`/var/lib/dbus/machine-id` 是其符号链接),可用 `machine-id + app-specific salt` 经 HKDF 派生 master key。machine-id 在 WSL2 下:`/etc/machine-id` 在 WSL distro 内稳定(重装 distro 才变),满足"DB 文件泄露 ≠ key 泄露"(攻击者拿到 DB 但拿不到这台机器的 machine-id,无法解密)。注意:machine-id 不是高强度秘密(本机任何用户可读 `/etc/machine-id`),但 PRD 已声明威胁模型**不防本机 root / 进程内存**,故 machine-id 绑定足够。

**若仍想用 keyring(作为 macOS/Windows 原生路径的锦上添花):**
1. 不要用 `keyring` facade 的 `default-features`(它在 Linux 会硬走 zbus secret-service)。
2. 直接依赖 `keyring-core`,按平台 cfg 选 store:`cfg(target_os="macos")` → `apple-native-keychain-store`;`cfg(target_os="windows")` → `windows-native-keyring-store`;**Linux 下不启用任何 store**(或启用 `db-keystore` 走加密文件,与主方案合流)。
3. 所有 keyring 调用必须 `match` `NoStorageAccess`/`NoDefaultStore`/`PlatformFailure` 并降级到应用层加密 fallback —— 永不 panic。
4. Windows 路径注意 secret 长度(issue #270,API key 一般无碍,但若将来存 OAuth token 需 base64 + 分片或改用 DPAPI)。

---

## 关键来源清单(可核验)

| 来源 | 用途 |
|---|---|
| https://docs.rs/keyring/latest/keyring/v1/ | v1 模块说明,Linux 走 Secret Service |
| https://docs.rs/keyring/latest/keyring/v1/enum.Error.html | Error 枚举(NoStorageAccess / NoDefaultStore),证明可捕获不 panic |
| https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/main/src/v1.rs | `Entry::new` 初始化逻辑(`if let Ok(store) = ...` 静默失败) |
| https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/main/Cargo.toml | feature → store crate 映射(v1 默认 Linux = zbus-secret-service) |
| https://docs.rs/linux-keyutils-keyring-store/latest/linux_keyutils_keyring_store/ | keyutils 后端说明 + **"不持久化跨重启"** 警告 |
| https://docs.rs/dbus-secret-service-keyring-store/latest/dbus_secret_service_keyring_store/ | **"Usage on Windows Subsystem for Linux"** 小节(WSL 无 default collection)+ "Headless usage" workaround |
| https://github.com/open-source-cooperative/keyring-rs/issues/133 | WSL systemd 下只有 session collection,维护者解释 WSL 无 PAM 故无 default collection,建议用 Windows build |
| https://github.com/open-source-cooperative/keyring-rs/issues/15 | 早期 WSL panic(用户 unwrap),根因 D-Bus 无 X11 无法 autolaunch |
| https://github.com/open-source-cooperative/keyring-rs/issues/83 | Linux 远程 VM 上 `PlatformFailure: I/O error`,error 非 panic |
| https://github.com/open-source-cooperative/keyring-rs/issues/266 | keyutils 后端 `set_password returning NoEntry` 竞态(gnome 47) |
| https://github.com/open-source-cooperative/keyring-rs/issues/270 | Windows Credential Manager 长 secret(~360 char)失败 |
| 本机实测 `/mnt/wslg`, `dbus-daemon`, `gnome-keyring-daemon`(缺失), `/etc/machine-id` | 2026-06-24 WSL2 环境实证 |

---

## Caveats / Not Found

- **未实测**在 WSL2 装 gnome-keyring + dbus-run-session + 创建 default collection 的完整链路(本轮纯调研,未在本机装 GNOME 依赖)。若最终方案需要,keyring 维护者建议的"启动时幂等创建 default collection"代码片段需另行 spike。
- **未查** Tauri 生态同类 app(VSCode Remote、1Password CLI 等)在 WSL 下的具体做法 —— 另一个 research topic(`industry-api-key-storage.md`)应覆盖。
- **db-keystore**(`keyring` 提供的"加密 SQLite store")未深入读源码,但从描述看是"应用自带加密 DB",与本项目"应用层加密 + 主 SQLite"方案高度重合,可能直接复用思路。
- `dbus-secret-service-keyring-store` 文档里"Usage on Windows Subsystem for Linux"小节引用的"this issue on GitHub"未带链接(文档原文如此),结合 #133 维护者讨论可确定指 #133。
- GitHub Search API 在本环境无认证 token 下返回 rate-limit 错误,改用 HTML 页面解析 issue 列表 + 单条 issue API(未触发限流),WSL 相关 issue 列表可能不全;但官方文档的 WSL 专节 + #133 维护者结论已是权威证据,不影响结论。
