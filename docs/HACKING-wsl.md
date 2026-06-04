# HACKING-wsl: WSL + Ubuntu 22.04 环境坑笔记

> 本机环境:WSL 2 (`6.6.114.1-microsoft-standard-WSL2`) + Ubuntu 22.04.2 LTS,linuxbrew 装在 `/home/linuxbrew/`,以 `carlos` 用户运行(`root` 是 sudo 临时升的)。
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

**修法**(一次性):
```bash
# 1. 装 fcitx5 + 拼音
sudo apt install -y fcitx5 fcitx5-chinese-addons fcitx5-frontend-gtk3

# 2. 预置 pinyin 为默认输入法(IM name 区分大小写都可以,但 profile 里写小写更稳)
mkdir -p ~/.config/fcitx5
cat > ~/.config/fcitx5/profile <<'EOF'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=pinyin

[Groups/0/Items/0]
Name=pinyin
Layout=

[GroupOrder]
0=Default
EOF

# 3. shell rc 里加 env + autostart(注意:必须 --enable pinyin,因为 pinyin 是 on-demand addon,
#    默认不会自动加载;profile 引用它,但加载时机在 profile 之后,鸡生蛋)
cat >> ~/.zshrc <<'EOF'
# IME env (fcitx5) for WSLg / native Linux apps including Tauri WebKit
export GTK_IM_MODULE=fcitx
export QT_IM_MODULE=fcitx
export INPUT_METHOD=fcitx5
export SDL_IM_MODULE=fcitx
export XMODIFIERS=@im=fcitx

# auto-start fcitx5
if [ -z "$FCITX5_AUTOSTARTED" ] && command -v fcitx5 >/dev/null 2>&1; then
  export FCITX5_AUTOSTARTED=1
  fcitx5 -d --enable pinyin >/dev/null 2>&1
fi
EOF
# bashrc 同上(略)

# 4. 启动 fcitx5
fcitx5 -d --enable pinyin
```

**注意**:
- WSLg Wayland socket 是 `/mnt/wslg/runtime-dir/wayland-0`,owner 是 `carlos`,**root 看不到**
- 任何用 `sudo` 跑 fcitx5 会立刻挂("All display connections are gone"),必须在你的 user 下启动
- env 变量必须进**交互式** shell 的 rc(.zshrc / .bashrc),不能靠 systemd(WSLg 的 systemd 不一定在)
- **坑中坑**:`pinyin` 是 `OnDemand=True` 的 addon,默认不加载。光在 profile 写 `DefaultIM=pinyin` 不够,必须 `--enable pinyin` 显式启用,否则 fcitx5 启动时打:
  ```
  W inputmethodmanager.cpp:96] Group Item Pinyin in group Default is not valid. Removed.
  ```
  然后用 keyboard-us 替代。
- 想看 fcitx5 现在有什么 IM:`fcitx5-diagnose` 跑一下,搜 "## Input Methods" 段

**验证**:
- `fcitx5 -d --enable pinyin` 不退码
- `fcitx5-diagnose` 的 "## Input Methods" 段显示 `DefaultIM=pinyin`(不是 keyboard-us)
- `ps aux | grep fcitx5` 看到进程在
- Tauri app 打开,点 textarea,打 `n` 出候选窗
- `fcitx5-config-qt` 也能跑(可在里面加/删输入法)

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

**现象**:fcitx5 起了,候选窗出得来,但按 **Ctrl+Space** / **Ctrl+Shift** 都切不动,Windows 右下角的 IME 指示器倒是有响应(被 Windows 切走了)。Shift+Space 默认也不通(WSLg 透键盘事件不完整)。

**根因**:
- WSLg 把键盘事件从 Windows 转给 Linux app 时,Windows 的全局 IME 切换热键(Ctrl+Space)会先被 Windows 自己吃掉,fcitx5 收不到
- Ctrl+Shift 在 Windows 上是"切换输入法",同样被吞
- 默认 fcitx5 的 `TriggerKey = Ctrl+space`、`AltTriggerKey = Shift+space`,前者跟 Windows 冲突

**修法**:改 fcitx5 的 hotkey 到不冲突的键。`~/.config/fcitx5/config` 写:

```ini
[Hotkey]
TriggerKeys[0]=Shift+space
AltTriggerKeys[0]=Shift+Shift_L+grave
EnumerateForwardKeys[0]=Control+Shift+Right
EnumerateBackwardKeys[0]=Control+Shift+Left
```

解释:
- `TriggerKeys[0]`:开/关 fcitx5(原 Ctrl+Space)— 改成 Shift+Space
- `AltTriggerKeys[0]`:在 pinyin / keyboard-us 之间切(原 Shift+Space)— 改成 Shift+Shift_L+反引号,跟 Caps Lock 误触也错开
- `EnumerateForward/Backward`:循环切所有 IM,改成 Ctrl+Shift+左右,跟 Windows 输入法切左右冲突
- 改完 `fcitx5-remote -r` 重载,**不**用重启 fcitx5 daemon

**验证**:
- fcitx5-remote 还能用 → reload 成功
- 在 Tauri 窗口点 textarea
- 按 Shift+Space → 候选窗消失,光标处直接出英文
- 再按 Shift+Space → 候选窗回来,可以打拼音
- 按 Shift+Shift_L+\` → 状态在 pinyin ↔ keyboard-us 之间切(可以看右下角指示器或者看 fcitx5-remote 输出)

**注意**:
- 这个 config 是 per-user 的(`~/.config/fcitx5/config`),carlos 和 root 各自有
- 我已经写好了 root 的(/root/.config/fcitx5/config),你 source 之后让 fcitx5 -r 重读就行
- 想要图形化配置:`fcitx5-config-qt`(在 WSLg 启个终端跑)

---

## 坑 1:linuxbrew 的 pkg-config 不搜系统路径

**现象**:`pkg-config --modversion webkit2gtk-4.1` 报 not found,即使 `apt install libwebkit2gtk-4.1-dev` 装过了。`ls /usr/lib/x86_64-linux-gnu/pkgconfig/` 能看到 `webkit2gtk-4.1.pc`。

**根因**:linuxbrew 的 pkg-config 把搜索路径**完全覆盖**到 `/home/linuxbrew/.linuxbrew/{lib,share,...}/pkgconfig`,不搜系统标准路径。

**修法**(持久):
```bash
# 加到 ~/.bashrc 和 ~/.zshrc
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"
```

**验证**:
```bash
pkg-config --modversion webkit2gtk-4.1   # 应返回 2.50.x
```

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

## 一次性环境脚本(把上面 5 个坑打包)

新 WSL 机器 / 重装时:

```bash
# 系统包
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  fonts-noto-cjk

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

# pnpm 死代理(如碰到)
pnpm config delete proxy
pnpm config delete https-proxy

# Rust 升级(以 carlos 跑,因为 brew 不让 root)
su carlos -c 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)" && brew upgrade rust'
```

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

# Node / pnpm
node --version    # 期望 >= 18
pnpm --version

# WSLg
ls /mnt/wslg      # 应存在
echo $DISPLAY     # 应 :0
echo $WAYLAND_DISPLAY  # 应 wayland-0
```

---

## 关联文档

- [spike-001](./spikes/001-wsl-tauri-window.md) — 这些坑的来源 spike
- [HACKING-llm.md](./HACKING-llm.md) — LLM API 兼容层差异(配对文档)
