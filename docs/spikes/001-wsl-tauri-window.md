# spike-001: WSL + Tauri 窗口显示

**日期**: 2026-06-XX(待填)
**状态**: 待执行 / 通过 / 失败-回退 / 失败-终止
**依赖**: 无
**预估耗时**: 1-3 小时(从环境到跑通)

## 目标

验证 Tauri 2 在 WSLg 下能编译、显示窗口、中文渲染正常。这是步骤 1 的**前置硬依赖**,失败则整套方案重选(退路顺序见下文)。

## 通过标准

### 硬通过(全部满足才进步骤 1)
- ✅ `cargo tauri dev` 启动 < 30 秒
- ✅ 窗口在 Windows 桌面正常显示
- ✅ 中文 / Emoji 渲染正常(不乱码、不方块)
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

## 关联文档

- [DESIGN §4 WSL 优先](./../DESIGN.md#4-决策wsl-优先windows-次要)
- [IMPLEMENTATION §2.5 步骤 5 WSL 体验](./../IMPLEMENTATION.md#25-步骤-5--wsl-体验-mvp)
- [REVIEW-deepseek-v4-pro §4.1 WSLg 兼容性](./../REVIEW-deepseek-v4-pro.md)
- [REVIEW-glm-5.1 §2.1 WSL 验证是最大定时炸弹](./../REVIEW-glm-5.1.md#21--wsl-验证是最大的定时炸弹)
