# HACKING-wsl: WSL + Ubuntu 22.04 环境坑笔记

> **本机环境**(截至 **2026-08-11** 更新):WSL 2 (`6.6.114.1-microsoft-standard-WSL2`) + Ubuntu 22.04.2 LTS,linuxbrew 装在 `/home/linuxbrew/`,以 `carlos` 用户运行(`root` 是 sudo 临时升的)。
>
> 写给未来的自己(或下个 session),撞到类似问题能 30 秒定位。
>
> **触发场景**:任何在 WSL 内做 Tauri / Rust / Node / pnpm 开发,第一次装环境或怀疑环境有问题时。

---

## 坑 6:WSL 没装中文输入法,Tauri WebKit 输不了中文

**现象**:Tauri 窗口里 textarea / input 点进去,按字母键出不来候选窗/选不到字,中文进不去。英文 OK。

**根因**:
- WSLg 把 Windows 键盘事件传进 Linux,但 Linux 原生 app(WebKitGTK 算一个)需要 WSL 侧有自己的 IME 服务(fcitx/ibus)与 `GTK_IM_MODULE` 串起来
- 装 WSL 时**默认不带任何 IME 服务**(连 fcitx5 都没有)
- Windows 端的微软拼音/搜狗对 WSLg 里的 Linux app 无效

**修法**(一次性,共 5 步,缺一不可):

```bash
## 1. 装 fcitx5 + 拼音 + GTK 前端
sudo apt install -y fcitx5 fcitx5-chinese-addons fcitx5-frontend-gtk3 fcitx5-frontend-gtk4

## 2. 更新 GTK3 immodules 缓存(坑 9)
# 装完 fcitx5 后,GTK3 的 immodules.cache 里可能没有 fcitx5 条目,
# 导致 GTK3 找不到 fcitx5 的 IM 模块,WebView 输入框无法连接 fcitx5。
sudo /usr/lib/x86_64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache
# 验证:输出应包含 fcitx 条目
/usr/lib/x86_64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 | grep -c fcitx  # 期望 > 0

## 3. 配置 fcitx5 profile:必须同时有 keyboard-us + pinyin(坑 10)
mkdir -p ~/.config/fcitx5
cat > ~/.config/fcitx5/profile <<'EOF'
[Groups/0]
# Group Name
Name=Default
# Layout
Default Layout=us
# Default Input Method
DefaultIM=pinyin

[Groups/0/Items/0]
# Name
Name=keyboard-us
# Layout
Layout=

[Groups/0/Items/1]
# Name
Name=pinyin
# Layout=
Layout=

[GroupOrder]
0=Default
EOF

## 4. shell rc 里加 DBus + env + autostart
# ---- fish (本机 ~/.config/fish/config.fish) ----
# DBus session bus (WSL 默认没起,fcitx5 和 WebKitGTK 都需要)
# 见下方 fish 配置片段
# ---- bash/zsh ----
cat >> ~/.zshrc <<'EOF'

# DBus session bus (WSL 默认没起,fcitx5 和 WebKitGTK 都需要)
if ! pgrep -f "dbus-daemon --session" >/dev/null 2>&1; then
  rm -f /run/user/$(id -u)/bus
  dbus-daemon --session --address=unix:path=/run/user/$(id -u)/bus --nofork >/dev/null 2>&1 &
  for _ in $(seq 10); do [ -S /run/user/$(id -u)/bus ] && break; sleep 0.2; done
fi
[ -S /run/user/$(id -u)/bus ] && export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus
export XDG_RUNTIME_DIR=/run/user/$(id -u)

# IME env (fcitx5) for WSLg / native Linux apps including Tauri WebKit
export GTK_IM_MODULE=fcitx
export QT_IM_MODULE=fcitx
export INPUT_METHOD=fcitx5
export SDL_IM_MODULE=fcitx
export XMODIFIERS=@im=fcitx

# auto-start fcitx5 (--keep:WSLg 下不会自杀; --enable pinyin:on-demand addon; --disable wayland:WSLg 不稳)
if [ -z "$FCITX5_AUTOSTARTED" ] && command -v fcitx5 >/dev/null 2>&1; then
  export FCITX5_AUTOSTARTED=1
  fcitx5 -d --keep --enable pinyin --disable wayland,waylandim >/dev/null 2>&1
fi
EOF

## 5. 启动 fcitx5
fcitx5 -d --keep --enable pinyin --disable wayland,waylandim
```

**fish shell 配置片段**(加入 `~/.config/fish/config.fish`):
```fish
# DBus session bus (WSL 默认没起,fcitx5 和 WebKitGTK 都需要)
if not test -S /run/user/(id -u)/bus; and command -v dbus-daemon >/dev/null 2>&1
    mkdir -p /run/user/(id -u); chmod 700 /run/user/(id -u)
    rm -f /run/user/(id -u)/bus
    dbus-daemon --session --address=unix:path=/run/user/(id -u)/bus --nofork >/dev/null 2>&1 &
    for i in (seq 10)
        test -S /run/user/(id -u)/bus; and break
        sleep 0.2
    end
end
if test -S /run/user/(id -u)/bus
    set -gx DBUS_SESSION_BUS_ADDRESS unix:path=/run/user/(id -u)/bus
    set -gx XDG_RUNTIME_DIR /run/user/(id -u)
end

# IME env (fcitx5) for WSLg / native Linux apps including Tauri WebKit
set -gx GTK_IM_MODULE fcitx
set -gx QT_IM_MODULE fcitx
set -gx INPUT_METHOD fcitx5
set -gx SDL_IM_MODULE fcitx
set -gx XMODIFIERS @im=fcitx

# auto-start fcitx5 (--keep:WSLg 下不会自杀; --enable pinyin:on-demand addon; --disable wayland:WSLg 不稳)
if not set -q FCITX5_AUTOSTARTED; and command -v fcitx5 >/dev/null 2>&1
    set -gx FCITX5_AUTOSTARTED 1
    fcitx5 -d --keep --enable pinyin --disable wayland,waylandim >/dev/null 2>&1
end
```

**注意**:
- WSLg Wayland socket 是 `/mnt/wslg/runtime-dir/wayland-0`,owner 是 `carlos`,**root 看不到**
- 任何用 `sudo` 跑 fcitx5 会立刻挂("All display connections are gone"),必须在你的 user 下启动
- env 变量必须进**交互式** shell 的 rc(.zshrc / .bashrc / config.fish),不能靠 systemd(WSLg 的 systemd 不一定在)
- **坑中坑**:`pinyin` 是 `OnDemand=True` 的 addon,默认不加载。光在 profile 写 `DefaultIM=pinyin` 不够,必须 `--enable pinyin` 显式启用,否则 fcitx5 启动时打:
  ```
  W inputmethodmanager.cpp:96] Group Item Pinyin in group Default is not valid. Removed.
  ```
  然后用 keyboard-us 替代。
- **坑中坑(2)**:fcitx5 在 WSLg 下必须加 `--keep` + `--disable wayland,waylandim`,否则启动后秒退("All display connections are gone")。WSLg 的 Wayland 协议实现不够完整,fcitx5 的 wayland addon 连不上。
- **坑中坑(3)**:没有 DBus session bus 时 fcitx5 能启动但 `fcitx5-remote` 会 crash("Failed to create dbus connection"),GTK app 也连不上 fcitx5。必须先起 DBus(见上方第 4 步)。

**中英文切换方式**:
- **按 Shift**(单独按一下):pinyin 内置的中英文切换,推荐,最方便
- **按 Super+Space (Win+Space)**:在 keyboard-us ↔ pinyin 之间切换(需配置 `~/.config/fcitx5/config`)
- Ctrl+Space 被 Windows IME 拦截,无效
- Ctrl+; 被 fcitx5 clipboard 插件占用,无效

**验证**:
- `fcitx5-remote` 不 crash,返回 0/1/2
- `fcitx5-diagnose` 的 "## Input Methods" 段显示 `keyboard-us` + `pinyin`
- Tauri app 打开,点 textarea,打 `n` 出候选窗
- 按 Shift 能在中英文之间切换

---

## 坑 7:WSL 默认以 root 启动,root 没 DBus session 也没 Wayland 访问

**现象**:WLS 默认登录就是 root(或 `sudo -i` 进 root shell);root 跑 fcitx5 起不来,报 "All display connections are gone, exit now";Tauri 倒能跑(它用 XWayland 走 DISPLAY=:0),但 fcitx5 找不到 DBus session 注册。

**根因**(WSLg 的 per-user 隔离):
- WSLg 的 Wayland socket `/mnt/wslg/runtime-dir/wayland-0` 绑了第一个登录的 user(carlos)
- root 的 `/run/user/0/` 目录是空的,**没有自己的 DBus session bus**
- fcitx5 走 DBus 跟客户端通信,root 没 bus → 客户端找不到 fcitx5
- fcitx5 默认加载 wayland/waylandim addon 想接 Wayland,root 接不到 carlos 的 socket → fcitx5 自杀

**修法**(root 专属,跟坑 6 配对):

```bash
# 1. /root/.zshrc(或 /root/.bashrc)加 DBus session bus 自启 + fcitx5 禁 wayland 前端
cat >> /root/.zshrc <<'EOF'

# root 用户 DBus session(WSL 下默认没起)
# 用 pgrep 查 dbus-daemon 实际在不在(避免 /run/user/0/bus 留僵尸 socket 误导)
if pgrep -f "dbus-daemon --session --address=unix:path=/run/user/0/bus" >/dev/null 2>&1; then
  [ -S /run/user/0/bus ] && export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
elif [ "$EUID" = "0" ] && command -v dbus-daemon >/dev/null 2>&1; then
  mkdir -p /run/user/0
  chmod 700 /run/user/0
  rm -f /run/user/0/bus  # 清任何僵尸 socket
  dbus-daemon --session --address=unix:path=/run/user/0/bus --nofork >/dev/null 2>&1 &
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -S /run/user/0/bus ] && break
    sleep 0.2
  done
  [ -S /run/user/0/bus ] && export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
fi
export XDG_RUNTIME_DIR=/run/user/0

# fcitx5 autostart(禁 wayland/waylandim,root 接不到 WSLg 的 wayland;
# 用 X11/XIM + GTK_IM_MODULE 跟 Tauri WebKitGTK-4.1 对话)
# pgrep -x 防多开
if command -v fcitx5 >/dev/null 2>&1 && ! pgrep -x fcitx5 >/dev/null 2>&1; then
  export FCITX5_AUTOSTARTED=1
  fcitx5 -d --keep --enable pinyin --disable wayland,waylandim >/dev/null 2>&1
fi
EOF
```

**注意**:
- `dbus-daemon` 起一个 root 专属的 session bus,写到 `/run/user/0/bus`(`XDG_RUNTIME_DIR` 标准位置)
- **坑中坑(2)**:用 `pgrep -f "dbus-daemon --session --address=unix:path=/run/user/0/bus"` 判断 daemon 活不活,不要只看 `[ -S /run/user/0/bus ]` — daemon 死掉会留僵尸 socket,fcitx5-remote 撞上去会 abort("Failed to create dbus connection")
- 必须在 `fcitx5` 之前启好(`for _ in ...; [ -S /run/user/0/bus ] && break; done` 等 socket)
- fcitx5 必须加 `--keep`,否则父 shell 一关就退(`-d` 模式下 fcitx5 监听主 display,root 的 display "不在"会自杀)
- fcitx5 必须 `--disable wayland,waylandim`,否则启动时试连 carlos 的 wayland socket 失败就 unload
- fcitx5 也用 `pgrep -x fcitx5` 防多开(每个 shell source rc 都想启一次,fcitx5 多个实例会抢 bus)
- env 全部从 rc 里 export,你的 Tauri 进程 fork 时会继承

**验证**:
```bash
# 1. dbus 起来了
ls -la /run/user/0/bus
# srwxrwxrwx 1 root root ... /run/user/0/bus

# 2. fcitx5 起来了 + pinyin 加载
ps aux | grep fcitx5 | grep -v grep
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus fcitx5-remote
# 应输出 0 (no client) 或 1/2,不能 abort

# 3. Tauri 起来后 WebKitGTK 能 connect
# 直接在 Tauri 窗口里打拼音测试

# 4. fcitx5-diagnose 看 ## Input Methods 段
fcitx5-diagnose | grep -A 5 "## Input Methods"
# 应显示 DefaultIM=pinyin
```

**carlos 也想跑怎么办**:carlos 的 rc 同样要加 `--disable wayland,waylandim`(`--keep` 也建议加),原因同 root,只是 carlos 的 wayland socket 是自己的所以不会失败,但 fcitx5 wayland addon 在 WSLg 上不稳。配置跟 root 完全一样就行。

---

## 坑 8:WSLg 下 Ctrl+Space / Ctrl+Shift 不能切 fcitx5 状态

**现象**:fcitx5 起了,候选窗出得来,但按 **Ctrl+Space** / **Ctrl+Shift** 都切不动,Windows 右下角的 IME 指示器倒是有响应(被 Windows 切走了)。

**根因**:
- WSLg 把键盘事件从 Windows 转给 Linux app 时,Windows 的全局 IME 切换热键(Ctrl+Space)会先被 Windows 自己吃掉,fcitx5 收不到
- Ctrl+Shift 在 Windows 上是"切换输入法",同样被吞
- Ctrl+; 被 fcitx5 clipboard 插件占用,触发剪贴板选择

**实际可用的切换方式**(无需额外配置):
- **按 Shift(单独按一下)**:pinyin 内置的中英文切换,最方便,实测在 WebKitGTK 中可用
- Super+Space(Win+Space):fcitx5 TriggerKey,可在 keyboard-us ↔ pinyin 之间切

**修法**(可选,配 Super+Space 触发键):`~/.config/fcitx5/config` 写:

```ini
[Hotkey]
TriggerKeys[0]=Super+space
EnumerateForwardKeys[0]=Control+Shift+Right
EnumerateBackwardKeys[0]=Control+Shift+Left
```

- 改完 `fcitx5-remote -r` 重载,**不**用重启 fcitx5 daemon
- 想要图形化配置:`fcitx5-config-qt`(在 WSLg 启个终端跑)

---

## 坑 9:装完 fcitx5 后 GTK3 immodules 缓存没更新,Tauri 连不上

**现象**:`fcitx5` 装了,进程在跑,`GTK_IM_MODULE=fcitx` 也设了,但 Tauri 窗口里 fcitx5 完全不工作 — 不出候选窗,不响应任何 IME 切换。`fcitx5-remote` 返回 0(无客户端连接)。

**根因**:GTK3 通过 `immodules.cache` 文件查找 IM 模块。这个缓存文件在安装系统时生成,装 `fcitx5-frontend-gtk3` 时如果没有触发 `postinst` 刷新缓存(Ubuntu 22.04 的 fcitx5 包有时不触发),缓存里就没有 fcitx5 条目。GTK3 启动时读缓存,找不到 fcitx5 模块,静默回退到默认 IM(什么都没有),**不报任何错误**。

**修法**(一次性):
```bash
sudo /usr/lib/x86_64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache
```

**验证**:
```bash
# 缓存里应有 fcitx5 条目(期望 > 0)
/usr/lib/x86_64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 | grep -c fcitx
# 或直接看缓存文件
grep fcitx /usr/lib/x86_64-linux-gnu/gtk-3.0/3.0.0/immodules.cache
```

**注意**:
- 模块文件本身在 `/usr/lib/x86_64-linux-gnu/gtk-3.0/3.0.0/immodules/im-fcitx5.so`,安装时就有,问题只是缓存没注册
- Tauri 2 链接的是 GTK3(`libgtk-3.so.0`),不是 GTK4,所以只需更新 GTK3 的缓存
- 这个坑特别隐蔽,因为没有任何报错 — GTK 只是静默忽略不认识的 IM 模块

---

## 坑 10:fcitx5 profile 只有 pinyin 一个输入法时,Shift 切换不生效

**现象**:fcitx5 能打中文,但按 Shift / 任何触发键都切不到英文。profile 里只写了 `pinyin`,没有 `keyboard-us`。

**根因**:fcitx5 的 profile Group 里只有一个输入法时,pinyin 内部的中英文切换逻辑异常 — 它不知道切换目标是什么。TriggerKey 是"在 Group 内的输入法之间轮流",只有一个输入法等于没得切。pinyin 内置的 Shift 切换也依赖 Group 里至少有两个输入法(或一个输入法 + keyboard 后备)。

**修法**:profile 里同时写 `keyboard-us` + `pinyin`:
```ini
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=pinyin

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=pinyin
Layout=

[GroupOrder]
0=Default
```

**注意**:
- 必须在 fcitx5 **未运行**时写这个文件,否则 fcitx5 退出时会用自己的内部状态覆盖回去
- 写完后再启动 fcitx5
- 加了 `keyboard-us` 后,**按 Shift**(单独按一下)就能在中英文之间切换

---

## 坑 11:Tauri 2 IPC arg 默认 `rename_all = "camelCase"`

**现象**:Rust 端 `async fn create_session(state: ..., project_id: String, initial_cwd: String, model: Option<String>)`,JS 端用 snake_case 调:
```ts
invoke("create_session", { project_id, initial_cwd, model: null })
```
报错:`Unhandled Promise Rejection: invalid args 'projectId' for command 'create_session': command create_session missing required key projectId`

**根因**:Tauri 2 IPC 边界对 Rust command 函数参数默认 `rename_all = "camelCase"` —— Rust 的 `project_id: String` 暴露给 JS 时是 `projectId`,`initial_cwd` 暴露是 `initialCwd`。JS 端用 snake_case 调用,key 找不到。

**修法**:JS 端 invoke 参数全用 camelCase:
```ts
invoke("create_session", { projectId, initialCwd })  // 正确
// 错误: { project_id, initial_cwd }
```

**特例**:单字参数(`path` / `id` / `fallback`)两种命名都接受,因为 snake_case / camelCase 形式一样。

**影响范围**:本项目所有 multi-word 参数的 Tauri command —— `list_sessions(project_id)` / `create_session(project_id, initial_cwd)` / `update_project_path(id, new_path)` / `update_project_name(id, new_name)` 等。详见 [docs/_history/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md](./_history/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md) §4.2 列表。

**验证**:写 PR 时,在 `check.jsonl` 加"Tauri command arg 是否 camelCase"作为验收硬约束。Spec 详见 [Tauri 2 命令参数命名约定](https://v2.tauri.app/develop/calling-rust/#optional-arguments)。

**经验沉淀**:这是 3b-1 PR2 实施的 3 个 hotfix 之一(post-fixes commit `18354a0` 修法 #1)。详见 [docs/_history/2026-06-3b-1/FOLLOW-UP.md FU-4](./_history/2026-06-3b-1/FOLLOW-UP.md#fu-4--tauri-2-ipc-arg-默认-rename_all--camelcase)。

---

## 坑 1:linuxbrew 的 pkg-config 不搜系统路径

**现象**:`pkg-config --modversion webkit2gtk-4.1` 报 not found,即使 `apt install libwebkit2gtk-4.1-dev` 装过了。`ls /usr/lib/x86_64-linux-gnu/pkgconfig/` 能看到 `webkit2gtk-4.1.pc`。**同样症状**:`cargo check` / `cargo test --lib` 在 `app/src-tauri/` 下报 `gdk-pixbuf-2.0` / `webkit2gtk-4.1` not found。

**根因**:linuxbrew 的 pkg-config 把搜索路径**完全覆盖**到 `/home/linuxbrew/.linuxbrew/{lib,share,...}/pkgconfig`,不搜系统标准路径。

**修法**(持久):
```bash
# 加到 ~/.bashrc 和 ~/.zshrc
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"
```

**一次性使用**(避免改 shell rc,适合 CI 或临时验证):
```bash
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
# 或
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
```

**注意**:**完整 Tauri runtime 需要 `pnpm tauri dev/build`**(走 `.cargo/config` 路径),`cargo test` 和 `cargo test --lib` 都需带 `PKG_CONFIG_PATH`,否则撞 `gdk-pixbuf not found`。

**验证**:
```bash
pkg-config --modversion webkit2gtk-4.1   # 应返回 2.50.x
```

**关联**:CLAUDE.md §Common Commands 同步记录了 `PKG_CONFIG_PATH=...` 的 cargo check / test 命令,与本坑修法等价。

---

## 坑 2:pnpm 配置了死代理

**现象**:`pnpm dlx` / `pnpm install` 报 `EHOSTUNREACH 192.168.0.160:7897`,但环境变量没设代理。

**根因**:`pnpm config` 里 `proxy` / `https-proxy` 字段配了死地址(可能之前代理用过,断了没清)。

**修法**:
```bash
pnpm config delete proxy
pnpm config delete https-proxy
```

**验证**:
```bash
pnpm config get proxy        # 应输出 undefined
pnpm config get https-proxy  # 应输出 undefined
pnpm dlx <anything>          # 不再 EHOSTUNREACH
```

---

## 坑 3:linuxbrew 装的 Rust 1.83 编不了现代 crate

**现象**:Cargo 编译时 `dlopen2_derive v0.4.3` 报 `feature 'edition2024' is required / not stabilized in this version of Cargo (1.83.0)`。多个 crate 需要 Rust 1.85+(dlopen2, getrandom, hashbrown)或 1.86+(icu_collections v2.2.0, deranged v0.5.8)。

**根因**:linuxbrew 的 `rust` formula 默认装 1.83(落后 stable 一年多)。`edition 2024` 在 Rust 1.85 才 stable。

**修法**(linuxbrew 升级,**不允许 root 跑 brew**):
```bash
# root 跑 brew 会拒绝
su carlos -c 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)" && brew upgrade rust'
# 验证
cargo --version   # 应是 1.96+ (Homebrew)
```

**更优选择(下次全新装时)**:直接装 **rustup** 而不是依赖 linuxbrew 的 rust。
- 切版本 / 装多版本更省事
- 跨项目固定 Rust 版本(用 `rust-toolchain.toml`)
- linuxbrew 只能升不能降,坑

---

## 坑 4:cargo package cache 锁冲突(全局 + 项目级 tauri-cli)

**现象**:`pnpm tauri dev` 卡在 `Blocking waiting for file lock on package cache`,不前进。

**根因**:同时跑 `cargo install tauri-cli`(全局) + `pnpm tauri dev`(项目级用 `@tauri-apps/cli` 2.11.2,装在 `node_modules/.bin/`),两个 cargo 进程争同一个 `~/.cargo/registry/cache/` 锁。

**修法**:**杀掉全局 install,只用项目级 CLI**。项目里 `package.json` 的 `devDependencies` 有 `"@tauri-apps/cli": "^2"`,`pnpm tauri dev` 就走它,完全够用,不需要全局装 `tauri-cli`。

**结论**:全局不装 tauri-cli。SPA 项目里 `pnpm tauri <cmd>` 等价于全局命令,还自动跟 `@tauri-apps/api` 版本对齐。

---

## 坑 5:WSLg 下 CJK 字体"看起来对齐但实际不齐"

**现象**:Tauri / WebKit 窗口里中文能显示、不乱码、不方块,但**中英文字号 baseline 不齐,有细微锯齿感**(subtle,容易漏)。长句"中文 ABC 中文"看着不规整。

**根因**:Ubuntu 默认装 `WenQuanYi Zen Hei`(文泉驿),小字号用位图,大字号才用矢量。WebKit 画中文时 fontconfig 把 `sans-serif` 默认指向 `DejaVu Sans`(英文字体)→ fallback 到 WenQuanYi → 中英文字体**字号、baseline 不一致**。

**修法两件套**:

1. 装 Noto Sans CJK SC(中英文字号对齐好,业界标准):
   ```bash
   sudo apt install fonts-noto-cjk
   fc-cache -fv
   ```

2. 写 `/etc/fonts/local.conf` 强制 `sans-serif:lang=zh` 优先 Noto Sans CJK SC(Ubuntu 默认 fontconfig 在 `lang=zh`(非 `lang=zh-cn`)时不走 Noto CJK 链,latent bug):
   ```xml
   <?xml version="1.0"?>
   <!DOCTYPE fontconfig SYSTEM "fonts.dtd">
   <fontconfig>
     <match target="pattern">
       <test name="lang" compare="contains">
         <string>zh</string>
       </test>
       <test name="family">
         <string>sans-serif</string>
       </test>
       <edit name="family" mode="prepend" binding="strong">
         <string>Noto Sans CJK SC</string>
       </edit>
     </match>
   </fontconfig>
   ```
   写完后再 `fc-cache -fv` 一次。

3. **杀 + 重启 Tauri 进程**:WebKit 启动时读 fontconfig,HMR 不会重读。所以 `pkill -f spike-app && pkill -f WebKit && pnpm tauri dev`,不是热重载能解决的。

**验证**:
```bash
fc-match "sans-serif:lang=zh"   # 应返回 Noto Sans CJK SC
fc-match "sans-serif:lang=zh-cn"  # 同上
```

**经验**:spike 验证视觉时,不仅看"有没有乱码",还要看"中英文 baseline 是否对齐"。看 Spipaste 截图最容易看出这种细微问题。

---

## 一次性环境脚本(把上面 11 个坑打包)

新 WSL 机器 / 重装时:

```bash
# 系统包
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  fonts-noto-cjk \
  fcitx5 fcitx5-chinese-addons fcitx5-frontend-gtk3 fcitx5-frontend-gtk4

# PKG_CONFIG_PATH
echo 'export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"' >> ~/.bashrc
echo 'export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"' >> ~/.zshrc

# fontconfig
sudo tee /etc/fonts/local.conf > /dev/null <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <match target="pattern">
    <test name="lang" compare="contains"><string>zh</string></test>
    <test name="family"><string>sans-serif</string></test>
    <edit name="family" mode="prepend" binding="strong">
      <string>Noto Sans CJK SC</string>
    </edit>
  </match>
</fontconfig>
EOF
fc-cache -fv

# GTK3 immodules 缓存(坑 9)
sudo /usr/lib/x86_64-linux-gnu/libgtk-3-0/gtk-query-immodules-3.0 --update-cache

# fcitx5 profile(坑 10:必须先停 fcitx5 再写)
pkill fcitx5 2>/dev/null; sleep 1
mkdir -p ~/.config/fcitx5
cat > ~/.config/fcitx5/profile <<'EOF'
[Groups/0]
# Group Name
Name=Default
# Layout
Default Layout=us
# Default Input Method
DefaultIM=pinyin

[Groups/0/Items/0]
# Name
Name=keyboard-us
# Layout
Layout=

[Groups/0/Items/1]
# Name
Name=pinyin
# Layout=
Layout=

[GroupOrder]
0=Default
EOF

# fcitx5 触发键(Super+Space,避免 Ctrl+Space 被 Windows 拦截)
cat > ~/.config/fcitx5/config <<'EOF'
[Hotkey]
TriggerKeys[0]=Super+space
EnumerateForwardKeys[0]=Control+Shift+Right
EnumerateBackwardKeys[0]=Control+Shift+Left
EOF

# GPU 渲染权限(消除 libEGL warning)
sudo usermod -aG render $(whoami)

# pnpm 死代理(如碰到)
pnpm config delete proxy
pnpm config delete https-proxy

# Rust 升级(以 carlos 跑,因为 brew 不让 root)
su carlos -c 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)" && brew upgrade rust'

# DBus + fcitx5 + IME env 加入 shell rc(见坑 6 的完整配置片段)
```

> **fish shell 用户**:DBus + IME env 配置见坑 6 的 fish 配置片段,加入 `~/.config/fish/config.fish`。

---

## 通用检查清单(怀疑环境有问题时)

```bash
# Rust 工具链
cargo --version
rustc --version

# webkit2gtk(给 Tauri 2 用)
pkg-config --modversion webkit2gtk-4.1   # 期望 2.50.x
pkg-config --modversion javascriptcoregtk-4.1  # 期望 2.50.x

# CJK 字体
fc-match "sans-serif:lang=zh"   # 期望 Noto Sans CJK SC
fc-list :lang=zh | wc -l         # 期望 > 0

# GTK3 IM 模块缓存(坑 9)
grep -c fcitx /usr/lib/x86_64-linux-gnu/gtk-3.0/3.0.0/immodules.cache  # 期望 > 0

# fcitx5
ps aux | grep fcitx5 | grep -v grep  # 期望有进程
fcitx5-remote                        # 期望返回 0/1/2,不 crash
cat ~/.config/fcitx5/profile | grep Name  # 期望有 keyboard-us + pinyin

# Node / pnpm
node --version    # 期望 >= 18
pnpm --version

# WSLg
ls /mnt/wslg      # 应存在
echo $DISPLAY     # 应 :0
echo $WAYLAND_DISPLAY  # 应 wayland-0
echo $GTK_IM_MODULE   # 应 fcitx
echo $XMODIFIERS      # 应 @im=fcitx
echo $DBUS_SESSION_BUS_ADDRESS  # 应 unix:path=/run/user/UID/bus
```

**daemon 健康检查(怀疑 daemon / 远程访问有问题时加跑)**:
```bash
# daemon 进程在不在 + health endpoint(期望 200 + camelCase JSON)
./scripts/daemon.sh status
curl -s http://localhost:7456/api/v1/health

# daemon 是否监听 0.0.0.0:7456(WSL 2 forwarding 需要它绑非 127.0.0.1)
ss -tlnp | grep 7456

# 看 daemon 日志(bg 模式写 /tmp/everlasting-daemon.log)
./scripts/daemon.sh logs
```

---

## 远程访问 daemon 部署(Phase 2,2026-07-23)

> 这是本项目的**核心使用场景**:WSL 跑 daemon,Windows 宿主浏览器访问。Phase 2 把 agent core 拆成独立 `everlasting-daemon` 进程(P2.2),前端默认走 httpTransport(P2.4 D3),daemon `ServeDir` 兜底提供前端(P2.4 D4)—— 单二进制即可在浏览器里跑完整功能。详见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md)。

### 推荐方式:`scripts/daemon.sh`(commit `a2bd611` 之后)

日常用项目自带的 `scripts/daemon.sh` 管理浏览器模式 daemon,它包好了「编译 + 启动 + PID 文件 + 日志 + health 检查」,不用手敲 `cargo build` 和 `--port`:

```bash
./scripts/daemon.sh start   [--port N]   # 编译 release + 前台启动(日志直接输出)
./scripts/daemon.sh bg      [--port N]   # 后台启动(日志写 /tmp/everlasting-daemon.log)
./scripts/daemon.sh stop                 # 停 daemon(读 PID 文件,SIGTERM→15s→SIGKILL 兜底)
./scripts/daemon.sh restart [--port N]   # stop + start(改完前端重新 serve dist)
./scripts/daemon.sh rebuild              # 只重新编译 release 二进制(不重启)
./scripts/daemon.sh status               # 显示 PID + health 检查
./scripts/daemon.sh logs                 # 跟踪日志(bg 模式的日志文件)
```

> ✅ **bg 模式已恢复(2026-08-24 gate 修复)**:daemon bin 的孤儿守卫
> `prctl(PR_SET_PDEATHSIG)`(2026-07-27 起,`928bdc3`)曾让 `daemon.sh bg` 必死 ——
> 脚本退出即 SIGTERM 杀 daemon。08-24 起守卫 gate 到 **sidecar 模式**
> (`EVERLASTING_SIDECAR=1`,GUI 的 sidecar.rs spawn 才注入),standalone
> (`daemon.sh bg` / `cargo run` / CI)不再设该 env,后台 daemon 被 init 收养正常存活;
> 防护 1(`getppid()==1` 拒启动)一并 gate(systemd/daemonize 场景 ppid 天然是 1)。
> 见 `bin/everlasting-daemon.rs` 的 gate 注释。

脚本细节:用 **PID 文件**(`.everlasting-daemon.pid`)管理进程,避免 `pkill -f everlasting-daemon` 自匹配(脚本命令行含 daemon 名会被自己误杀);自动注入 `PKG_CONFIG_PATH`(WSL 编译 Rust 前置,见坑 1);`--no-build` 跳过编译用现有二进制。设计文档与本节对应;脚本头注释是权威。

> ⚠️ **多实例反模式(Q1)**:**不要同时跑两个 daemon**(比如一个 `scripts/daemon.sh start` + 一个裸 `everlasting-daemon --port 7457`)。两个 daemon 各自打开(或试图打开)同一个 SQLite,会出现:① 写竞争 / `SQLITE_BUSY`;② 各自 `reap_orphaned_runs` 互踩;③ 如果落到不同 `data_dir`(见 [DEBUG_DB §1.0 孤儿 DB 坑](./DEBUG_DB.md#10-daemon-化后的三条解析路径2026-07-同步)),数据分裂成两份历史。要换端口做 A/B,先 `stop` 旧的。`scripts/daemon.sh` 用单 PID 文件做了防多实例,裸跑没有这层保护。

### detach 边界:后台任务的存活由 daemon 进程决定(F6,2026-08-27)

agent loop 是 daemon 进程里的 fire-and-forget tokio task —— **客户端断开绝不是取消源**(cancel 只有三个来源:Stop 按钮 / 破坏性命令 / daemon 停机)。由此两种「关闭」语义截然不同:

| 关闭方式 | 任务命运 | 交互 |
|---|---|---|
| **Web / PWA**:关标签(Ctrl+W)/ 杀 App / 锁屏 | **照常跑完**,结果落 DB,回来刷新可见 | 无弹窗(standalone daemon 独立存活,拦不住也不该拦) |
| **桌面 GUI(Tauri 壳)**:窗口 X / Alt+F4 / 任务栏关闭 | **全部终止**(GUI 退出回收 sidecar daemon) | 有在跑会话时弹确认「终止并关闭」;无在跑直接关 |

要真正的耐久后台(关掉一切窗口任务继续跑):用 `scripts/daemon.sh` 起独立 daemon,浏览器/PWA 访问。配套可观测性:`list_sessions` 的 `busy` 字段跨端可见哪些 session 在跑,轮次终结(非当前 session)有完成 toast(`app_config` `turn_complete_notify_enabled` 可关);跨 session 并发上限 `max_concurrent_loops`(缺省 4,改后需重启 daemon)。

### 调度边界:定时任务只在 daemon 进程跑(F2,2026-08-28)

`spawn_task_scheduler` 与 backup/sweeper 同款**只在 `bin/everlasting-daemon.rs` 装配**——GUI Full 模式(`?transport=tauri` 逃生)零 timer,**Settings「定时任务」面板可建/改任务但永不触发**(面板顶部有提示)。30s tick;停机跨过 fire 点重启后补跑一次(`last_fired_at` 判定);显式 disable→enable 不补跑存量。全局 kill switch `app_config` `scheduled_tasks_enabled`(fail-open)。

### 生产模式(裸二进制,手动部署)

daemon 自己 serve 前端 + API,同源无 CORS,Windows 宿主浏览器 `http://localhost:7456` 直达。

```bash
# 1. 构建前端 dist/
cd app && pnpm build

# 2. 构建 daemon release 二进制(含 sidecar staging)
#    2026-08-11 workspace 翻转后,产物统一落在**仓库根** target/release/
#    (不再在 app/src-tauri/target/)。根目录裸 `--bin` 只搜默认 member,
#    daemon 必须 `-p everlasting`(或在 app/src-tauri/ 内跑裸 `--bin`)。
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo build --release -p everlasting --bin everlasting-daemon

# 3. 启 daemon(监听 0.0.0.0:7456,Windows 宿主经 WSL 2 localhost forwarding 可达;
#    从仓库根跑,`resolve_dist_dir` 会沿 `app/src-tauri` 标记找到 app/dist 伺服前端)
./target/release/everlasting-daemon --port 7456
```

Windows 宿主(PowerShell 或浏览器):
```powershell
# 健康检查(期望 200 + camelCase JSON)
curl http://localhost:7456/api/v1/health

# 浏览器打开 —— 完整功能(发消息 / 流式 / permission / subagent)
start http://localhost:7456
```

### dev 模式(前后端分离)

vite dev server(1420)热更新前端,daemon(7456)跑 API;浏览器经 `?daemonUrl=` 跨域指向 daemon。

```bash
# 终端 1:daemon
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo run --bin everlasting-daemon -- --port 7456

# 终端 2:vite dev server
cd app && pnpm dev
```

浏览器:`http://localhost:1420?daemonUrl=http://localhost:7456`
- vite 1420 serve 前端(热更新);`?daemonUrl=` 把 httpTransport 指向 daemon 7456(跨域,daemon 的 `CorsLayer::very_permissive` 放行)。
- 也可 `pnpm dev:all`(`concurrently` 同时起两者,见 `app/package.json`)。

### 降级排查:Windows 宿主访问不通 daemon

WSL 2 默认有 localhost forwarding(Windows 宿主 `localhost:7456` → WSL 内 `0.0.0.0:7456`),但某些 Windows 版本 / 网络配置下会失效。按顺序排查:

**1. daemon 是否在监听 + 监听地址**
```bash
# WSL 内
ss -tlnp | grep 7456          # 期望 LISTEN 0.0.0.0:7456(daemon 绑 0.0.0.0,非 127.0.0.1)
curl http://localhost:7456/api/v1/health   # WSL 内自测,期望 200
```
若只监听 `127.0.0.1`,检查 daemon bin 是否传了正确 `--port`(server.rs `serve_daemon` 固定绑 `0.0.0.0`,无需改)。

**2. WSL 2 localhost forwarding**
```powershell
# Windows 宿主 PowerShell
curl http://localhost:7456/api/v1/health
```
不通 → forwarding 失效,走降级(3 或 4)。

**3. WSL 虚拟 IP(直连)**
```bash
# WSL 内取虚拟 IP
ip -4 addr show eth0 | grep -oP '(?<=inet\s)\d+(\.\d+){3}'
# 例:172.x.x.x
```
Windows 浏览器直接访问 `http://172.x.x.x:7456`(需 daemon 已绑 `0.0.0.0`,见 1)。

**4. netsh portproxy(Windows 侧端口转发,管理员 PowerShell)**
```powershell
# 把 Windows localhost:7456 转发到 WSL 虚拟 IP
netsh interface portproxy add v4tov4 listenport=7456 listenaddress=0.0.0.0 `
  connectport=7456 connectaddress=<上一步的 WSL IP>

# 验证
netsh interface portproxy show v4tov4
curl http://localhost:7456/api/v1/health

# 清理(不需要时)
netsh interface portproxy delete v4tov4 listenport=7456 listenaddress=0.0.0.0
```
> WSL 重启后虚拟 IP 可能变,需重新 `connectaddress`。生产用建议固定 WSL IP(`wsl.conf` 静态 IP)或 systemd 服务化。

### Tauri GUI 的 sidecar 模式(P2.4)

`pnpm tauri dev` / `pnpm tauri build` 时,GUI 进程自动 spawn `everlasting-daemon` sidecar(tauri-plugin-shell),前端默认走 httpTransport(同源 sidecar),无需手动起 daemon。关 Tauri 窗口自动 SIGTERM sidecar(`RunEvent::Exit` 钩子)。

- **逃生通道**:GUI 启动时带 `?transport=tauri` 走原 in-process AppState(Full 模式,daemon 不稳时回退)。
- **瘦客户端**(默认):GUI 不开 SqlitePool(`AppState::load` 不调),`lsof -p <gui-pid> | grep sqlite` 应空。

### 验证命令速查

```bash
# daemon 健康(WSL 或 Windows 宿主均可,经 forwarding)
curl http://localhost:7456/api/v1/health
# 期望:{"daemonId":"...","daemonVersion":"0.1.0","apiVersions":["v1"],"uptimeSeconds":N,"sessionCount":...}

# daemon 是否监听 0.0.0.0(WSL 内)
ss -tlnp | grep 7456

# WSL 虚拟 IP
ip -4 addr show eth0 | grep -oP '(?<=inet\s)\d+(\.\d+){3}'
```

---

## 测试性能(WSL 后端 cargo test)

> **背景**:本机 12 核 / 11G 内存(WSL 2 swap 常满)。`--lib` 套件用例数**随测试增涨**(remote epic 又新增 tunnel 单元 + e2e 测试),**以 `cargo test` 输出为准**;下表耗时是 2026-08-05 的实测基线(rustc 1.96):

| 阶段 | 耗时 | 备注 |
|---|---|---|
| `cargo test --no-run`(冷编译) | ~1m37s | 全量编 4 个 test binary |
| `cargo test --no-run`(增量) | ~11s | 改完代码重测的实际成本 |
| `cargo test --lib`(默认多线程 ≈ 6 线程) | **26.3s** | 全量用例通过(数量随测试增涨,以 `cargo test` 输出为准) |
| `cargo test --lib`(`--test-threads=1`) | **72.1s** | 仅用于逐用例计时 |
| `cargo test --test e2e` | 4.2s 跑测 / 26.7s 含编译 | 10 个 e2e |

### 建议:默认多线程跑,别逐模块串行

**坑(踩过)**:想测某个模块的耗时,直觉写法是

```bash
for m in agent llm tools db ...; do
  cargo test --lib "$m::" 2>&1 | tail   # ❌ 每次都重新链接 + spawn test binary
done
```

**每次** `cargo test` 调用都要付 ~11s 的 relink + 进程启动税,16 个模块串下来光 baseline 就 ~180s,真实测试时间反而被淹没。实测会看到 `agent` 模块"耗时 630s"的假象(其实是 16 次串行 baseline 累加),而一次性 `cargo test --lib` 只要 26s。

**正确做法**:
- **日常跑测**直接 `cargo test --lib`(默认就是多线程,thread 数 = CPU 核数),**不要**自己加 `--test-threads=1`。
- **找慢用例**用 [`cargo-nextest`](https://nexte.st)(`cargo nextest run --lib`),它原生带逐用例计时 + 可并行;本机离线装不上时,退而求其次:单线程跑一次 `cargo test --lib -- --test-threads=1` 并对每行输出打时间戳(见下方脚本),相邻两行的时间差 ≈ 该用例耗时。
- **冒烟**用模块 filter 缩范围:`cargo test --lib "agent::tests_agent_loop::"`,**一次** `cargo test` 调用内用 filter,而不是循环调多次。

```bash
# 单线程逐用例计时(无 nextest 时的 fallback;lib binary hash 随编译变,先 --no-run 拿到)
# 2026-08-11 workspace 翻转后 test 产物在**仓库根** target/debug/deps,必须
# 从根跑 + `-p everlasting`(根目录裸 `--lib` 只测 default members,测不到 daemon)。
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib --no-run  # 得到 target/debug/deps/everlasting_lib-<hash>
BIN=target/debug/deps/everlasting_lib-<hash>
"$BIN" --test-threads=1 2>&1 | awk '
  /^test .* \.\.\. (ok|FAILED|ignored)/ {
    "date +%s.%N" | getline ts; close("date +%s.%N")
    print ts, $0
  }
' | awk 'NR>1 { printf "%8.3f %s\n", $1-prev, substr($0, index($0,$2)) } { prev=$1 }' \
  | sort -rn | head
```

### 已知最耗时用例(单线程计时,2026-08-05)

| 耗时 | 用例 | 根因 |
|---|---|---|
| **14.2s** | `agent::tests_agent_loop::agent_loop_max_turns_emits_done_marker` | 跑满 `MAX_TURNS=200` 轮 mock loop(每轮 resolver poll sleep 5+2ms) |
| 7.5s | `daemon::server::tests::serve_daemon_shutdown_drains_active_agent_loop` | 真实 SIGTERM + 等 `SHUTDOWN_GRACE_SECS=3` |
| 5.3s | `daemon::server::tests::serve_daemon_keeps_serving_without_signal_past_grace_window` | 硬 `sleep(GRACE_SECS+2)=5s`,且受 `SIGNAL_TEST_MUTEX` 串行 |
| 2.9s | `agent::context::case_7_long_history_at_max_turns_compacts_safely` | 100 条消息 token 计数压缩(纯计算,无 sleep) |

> daemon 三个信号测试因 `libc::kill(getpid(), SIGTERM)` 是进程级信号,必须经 `SIGNAL_TEST_MUTEX` **串行**——多线程也救不了,这是设计约束不是 bug。

### 内存压力对并行度的限制

本机 swap 已满(11G RAM / 3G swap 全占),`cargo test --lib` 6 线程只拿到 ~2.7× 加速(72s→26s),远不到理论 6×。表现是编译/链接阶段大量换页。**临时缓解**:跑测前关掉 daemon / 浏览器 / 其他大内存进程;或显式压线程数 `cargo test --lib -- --test-threads=4`(避免 OOM 时被 kernel 杀 test 进程)。

---

## 关联文档

- [spike-001](./_history/spikes/001-wsl-tauri-window.md) — 这些坑的来源 spike
- [HACKING-llm.md](./HACKING-llm.md) — LLM API 兼容层差异(配对文档)
- [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md) — daemon 拆分 / 远程访问实施路线图(Phase 1/2/3)
- [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) — 云端 `everlasting-remote` 部署手册(systemd + nginx + 配对)
- [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md) — remote E2E 验收手册(配对 / PWA 手机访问)
- [`scripts/daemon.sh`](../scripts/daemon.sh) — 浏览器模式 daemon 管理脚本(start/bg/stop/restart/rebuild/status/logs),本节「推荐方式」的底层实现
- [`scripts/remote.sh`](../scripts/remote.sh) — remote 服务端管理脚本(本地起 / 状态,`deploy-remote.sh` 部署到国内 2C2G 服务器)
- [DEBUG_DB.md](./DEBUG_DB.md) — SQLite 直连调试;§1.0 有 daemon 视角的三条 DB 路径解析 + 孤儿 DB 坑
