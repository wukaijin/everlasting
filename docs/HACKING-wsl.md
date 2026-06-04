# HACKING-wsl: WSL + Ubuntu 22.04 环境坑笔记

> 本机环境:WSL 2 (`6.6.114.1-microsoft-standard-WSL2`) + Ubuntu 22.04.2 LTS,linuxbrew 装在 `/home/linuxbrew/`,以 `carlos` 用户运行(`root` 是 sudo 临时升的)。
>
> 写给未来的自己(或下个 session),撞到类似问题能 30 秒定位。
>
> **触发场景**:任何在 WSL 内做 Tauri / Rust / Node / pnpm 开发,第一次装环境或怀疑环境有问题时。

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
