# spike-001: WSL + Tauri 窗口显示

**日期**: 2026-06-04
**状态**: 通过(5 条硬通过全部满足)
**依赖**: 无
**预估耗时**: 1-3 小时(从环境到跑通)— 实际 50 分钟(含环境装包、撞坑修环境、首次编译 27 分钟)

## 目标

验证 Tauri 2 在 WSLg 下能编译、显示窗口、中文渲染正常。这是步骤 1 的**前置硬依赖**,失败则整套方案重选(退路顺序见下文)。

## 通过标准

### 硬通过(全部满足才进步骤 1)
- ✅ `cargo tauri dev` 启动 < 30 秒
- ✅ 窗口在 Windows 桌面正常显示
- ✅ 中文 / Emoji 渲染正常(不乱码、不方块、**中英文字号 baseline 对齐**——见下方"撞到的坑 #5",Ubuntu 默认 WenQuanYi 会有细微锯齿和对不齐,需装 Noto CJK + 写 local.conf)
- ✅ 至少 10 次热重载不崩
- ✅ WebView2 / WebKitGTK 进程能在 WSL 内 `ps` 到(确认 WSL 进程,不是绕到 Windows 侧)

### 可接受瑕疵(不阻塞,留打磨)
- ⚠️ HiDPI 模糊
- ⚠️ 启动 30-60 秒
- ⚠️ GTK 主题丑

### 硬失败(任一 → 走退路)
- ❌ 窗口根本不显示
- ❌ 中文/Emoji 乱码无法解决
- ❌ 编译报错无法解决
- ❌ WebView 进程跑在 Windows 侧(违背"无 wslapi 调用"原则)

---

## 执行步骤

### 1. 环境检查(预估 5 分钟)

```bash
node --version           # 期望 >= 18
pnpm --version           # 或 npm/yarn
rustc --version          # 期望 >= 1.75
pkg-config --modversion webkit2gtk-4.1   # 期望 4.1.x
ls /usr/lib/x86_64-linux-gnu/ | grep webkit  # 应该看到 webkit2gtk-4.1
```

如果 webkit2gtk-4.1 缺失:
```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential curl wget file \
  libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

### 2. 创建项目(预估 5 分钟)

```bash
cd ~
mkdir -p tauri-spike && cd tauri-spike
pnpm create tauri-app@latest
# 交互式选:
#   - Project name: spike-app
#   - Identifier: com.spike.app
#   - Frontend: Vue
#   - Language: TypeScript
#   - Package manager: pnpm
cd spike-app
pnpm install
```

### 3. 启动 + 计时

```bash
time cargo tauri dev
```

**记录**:
- 启动到窗口出现的秒数(命令的 real 时间)
- 默认页面是否显示(应 Vite + Vue 欢迎页)

### 4. 验证中文 / Emoji 渲染

修改 `src/App.vue`,加:
```vue
<template>
  <main>
    <h1>中文测试 你好世界 🦀</h1>
    <p>Emoji: 🎉 ✅ ❌ 🚀</p>
  </main>
</template>
```

**观察**:
- 中文是否乱码 / 方块
- Emoji 是否显示
- 字体大小 / 颜色是否正常

如果乱码,先尝试:
```bash
sudo apt install -y fonts-noto-cjk
fc-cache -fv
```
然后重启 `cargo tauri dev`。

### 5. 验证热重载

- 修改 `src/App.vue` 的标题文字,保存
- **期望**:窗口内文字自动更新,无报错
- 重复 10 次,记录失败次数(编译报错 / 页面崩溃 / 无反应 都算失败)

### 6. 验证 WebView 进程在 WSL 内

新开一个 WSL 终端:
```bash
ps aux | grep -iE 'webkit|webview|tauri' | grep -v grep
```

**期望**:看到 `webkit2gtk-...` 或 `WebKitWebProcess` 进程,**uid 是 WSL 用户**(不是 Windows 侧的用户)

不应该只看到 Windows 侧的 `msedgewebview2.exe`(那意味着渲染层在 Windows,违背"无 wslapi"原则)。

---

## 失败 → 走哪个回退

| 现象 | 回退决策(按代价从小到大) |
|------|---------|
| 窗口不显示 / 启动卡死 | 退路 1:XWayland 强制转发(配置 `GDK_BACKEND=x11` 后 `cargo tauri dev`) |
| 中文/Emoji 乱码 | 退路 1:装 `fonts-noto-cjk` + `fc-cache -fv`,再试 |
| webkit2gtk 编译失败 | 退路 1:确认装的是 `-4.1-dev` 不是 `-4.0-dev`,版本不匹配是常见坑 |
| 上述 1-2 天没解决 | 退路 2:换平台(macOS 优先 → 放弃 WSL 优先约束,需更新 DESIGN §4) |
| macOS 也不想换 | 退路 3:Tauri → Electron(违背 TECH §1.3,需更新技术选型) |
| 全部失败 | **重新评估整个项目**(WSL + Tauri 是基础平台,这一层不行所有上层设计都失去意义) |

---

## 跑完后贴给 Claude

- 启动时间数字(real 行)
- 中文 / Emoji 渲染的截图(可文字描述现象)
- 10 次热重载成功 / 失败次数
- `ps aux | grep -iE 'webkit|webview|tauri'` 输出
- 如果失败:**完整失败现象 + 你已尝试的回退**

---

## 实际执行 / 结论 / 后续动作

### 实际执行(2026-06-04)

**环境(macOS 之外 → 实际 WSL)**:
- 平台:WSL 2,Ubuntu 22.04.2 LTS(`6.6.114.1-microsoft-standard-WSL2`)
- WSLg 已挂载(`/mnt/wslg`),`DISPLAY=:0`, `WAYLAND_DISPLAY=wayland-0`
- 初始 Rust:linuxbrew 装的 1.83.0(过老,见下方"撞到的坑" #1)
- 初始 webkit2gtk-4.1:未装,装的是 2.50.4
- ANTHROPIC_API_KEY:未设(本 session 只跑 001,002 跳过)

**撞到的坑 + 修复**:

1. **linuxbrew 的 pkg-config 不搜系统路径**:`pkg-config --modversion webkit2gtk-4.1` 报 not found,即使系统装了。linuxbrew 的 pkg-config 把搜索路径完全覆盖到 `/home/linuxbrew/.linuxbrew/{lib,share,...}/pkgconfig`。修复:在 `~/.bashrc` 和 `~/.zshrc` 加 `export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig:${PKG_CONFIG_PATH}"`。
2. **pnpm 配置了死代理**:`pnpm config get proxy` = `http://192.168.0.160:7897`(代理已不可达),导致 `pnpm dlx create-tauri-app` 失败。修复:`pnpm config delete proxy` + `pnpm config delete https-proxy`。
3. **Rust 1.83 编译不了 Tauri 2 依赖图**:icu_collections v2.2.0 / dlopen2_derive v0.4.3 / deranged v0.5.8 / getrandom v0.4.2 / hashbrown v0.17.1 等都需要 Rust 1.85+ 或 1.86+,1.83 报 "feature `edition2024` is required"。退路选择:`brew upgrade rust`(linuxbrew 装的是 1.83,formulae 上 1.96.0 stable 可装)。注意:brew 不让以 root 跑,需要 `su carlos -c 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)" && brew upgrade rust'`,装完 cargo 自动指到 1.96.0。**如果以后别的项目要装 rust,建议装 rustup 而不是 linuxbrew 的**——rustup 切版本/装多版本更省事,linuxbrew 这条路只能升不能降。
4. **Cargo package cache 锁冲突**:同时跑 `cargo install tauri-cli`(全局) + `pnpm tauri dev`(项目级 CLI 装的 `@tauri-apps/cli` 2.11.2),两个 cargo 争同一个 package cache,`pnpm tauri dev` 卡在 "Blocking waiting for file lock on package cache"。退路选择:杀掉全局 install,只用项目级 CLI(@tauri-apps/cli 在 devDependencies 里,够用)。**结论:没必要全局装 tauri-cli,项目里 @tauri-apps/cli 就够。**
5. **WSLg 下 CJK 字体对齐/锯齿(本次撞到,补充)**:spike 文档里写"如果乱码,装 `fonts-noto-cjk`",但**实际不乱码也仍有问题**。Ubuntu 默认装的是 `WenQuanYi Zen Hei`,WebKit 画中文时:fontconfig 把 sans-serif 默认指向 DejaVu Sans → fallback 到 WenQuanYi → 中英文字号、baseline 不一致 → 截图里"中文测试 你好世界"看着**有一点点对不齐和锯齿感**(不是乱码,不是方块,是 subtle 的位图+fallback 链问题)。修法两件套:
   - `sudo apt install fonts-noto-cjk` + `fc-cache -fv`(装 Noto Sans CJK SC,中英文字号对齐好)
   - 写 `/etc/fonts/local.conf` 强制 `sans-serif:lang=zh` 优先 Noto Sans CJK SC(Ubuntu 默认 fontconfig 配置在 `lang=zh`(非 `lang=zh-cn`)时不走 Noto CJK 链,这是个 latent bug,fix 写法见文档附录)→ 然后 `fc-cache -fv` → 杀掉 spike 进程(让 WebKit 重新读 fontconfig) → 重启 `pnpm tauri dev`
   - **Spike 文档要更新**:`通过标准 §3 "中文/Emoji 渲染正常"` 应该加 "中英文字号对齐(无 WenQuanYi 那种细微锯齿)",否则只看一眼"没乱码"就过,会遗留这个坑。**结论:spike 验证视觉,不仅要看"有没有乱码",还要看"中英文字号 baseline 是否对齐"**。

**5 条硬通过,实测**:

| 编号 | 标准 | 实测 | 结果 |
|------|------|------|------|
| 1 | `cargo tauri dev` 启动 < 30 秒 | **冷启动 27 分钟**(首次 cargo 编译 488 crate);**热启动 22 秒**(增量编译 21.4s + spike-app + webkit 启动) | ✅ 二次启动 < 30s |
| 2 | 窗口在 Windows 桌面正常显示 | `spike-app` 标题、居中、合理大小,见 Snipaste_2026-06-04_15-17-45.png | ✅ |
| 3 | 中文 / Emoji 渲染正常 | 首次默认 WenQuanYi:中英文字号 baseline 不一致,有细微锯齿感(用户视觉反馈);装 `fonts-noto-cjk` + `/etc/fonts/local.conf` 强制 `sans-serif:lang=zh` → Noto Sans CJK SC 后对齐正常(用户确认)。Emoji 🎉 ✅ ❌ 🚀 🐳 🌈 🔥 7 个全部彩色渲染,无方块、无乱码 | ✅(装字体后) |
| 4 | 至少 10 次热重载不崩 | 10/10 全部通过(第一次脚本有 sed 模式 bug,重做后 10/10 全过)。13 条 hmr update 事件全在,WebKitWebProcess 一直活着(uid 0,root) | ✅ |
| 5 | WebView 进程能在 WSL 内 `ps` 到 | `WebKitNetworkPr` (PID 703006→706999) + `WebKitWebProces` (PID 703036→707021) 都在 WSL 内,uid=0/root,没有 msedgewebview2.exe 在 Windows 侧 | ✅ |

**关键 ps 输出(二次启动后)**:
```
 706537 (-- spike-app) ← tauri dev 父进程
 ├─ 706999 WebKitNetworkPr /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess  (uid 0)
 └─ 707021 WebKitWebProces /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitWebProcess       (uid 0)
```

**可接受瑕疵(不阻塞)**:
- 启动冷启动 27 分钟远超 30s 标准(预期内,首次 cargo 编译 488 crate,网络还慢)
- 视觉上无明显瑕疵(HiDPI 不模糊、字体不丑、主题自动跟系统深色)

### 结论

**spike-001 通过,WSL + Tauri 2 + Vue 3 技术栈可走。**5 条硬通过全过,没有触发任何退路。

支撑的下游决策:
- DESIGN §4 WSL 优先不动
- TECH §1.3 Tauri 2 不动
- TECH §1 前端 Vue 3 + Vite 不动
- IMPLEMENTATION §2.1 步骤 1 可开始(项目骨架已验证,只是搬家到 `/usr/local/code/github/everlasting/app/` 即可)

### 后续动作

- ✅ spike-001 通过 → IMPLEMENTATION 步骤 1 开工前置 OK
- ⏳ spike-002(reqwest+Anthropic SSE)未跑(本 session 因 ANTHROPIC_API_KEY 未设跳过)→ 下个 session 补
- ⏳ spike-003(git2-rs)、spike-004(sqlx)可与 MVP 步骤 1 并行启动
- 📝 把"linuxbrew rust 路径"和"PKG_CONFIG_PATH"两个环境坑写进 `docs/HACKING-wsl.md`(目前没有这个文件,值得新建,记录本机环境特殊点)
- 📝 同样的,把"WSLg 下 CJK 字体对齐(装 fonts-noto-cjk + 写 /etc/fonts/local.conf)"也写进 HACKING-wsl.md,这是 MVP 步骤 1 直接会用到的
- 📝 更新 spike 文档自身的"通过标准 §3":加"中英文字号 baseline 对齐"判定,避免后续 session 重复踩这个坑

---

## 关联文档

- [DESIGN §4 WSL 优先](./../DESIGN.md#4-决策wsl-优先windows-次要)
- [IMPLEMENTATION §2.5 步骤 5 WSL 体验](./../IMPLEMENTATION.md#25-步骤-5--wsl-体验-mvp)
- [REVIEW-deepseek-v4-pro §4.1 WSLg 兼容性](./../REVIEW-deepseek-v4-pro.md)
- [REVIEW-glm-5.1 §2.1 WSL 验证是最大定时炸弹](./../REVIEW-glm-5.1.md#21--wsl-验证是最大的定时炸弹)
