# Handoff — 新 Session 引导

> **你是新 session**。先读这份文档(5 分钟),再读 spike 001/002,再动手。
> 当前阶段:**关键技术验证**,不是编码。

---

## 1. 项目是什么

**Everlasting**:个人使用的 vibe coding 桌面工作台。基于 Tauri 2 + Vue 3 + Rust,WSL 优先。

**核心定位**:
- 给"在 WSL 里写代码的 Windows 用户"用的桌面应用
- 自研 agent core(学 harness engineering,不包装 SDK)
- 多项目 / 多 session / 多 channel(后续扩展)

**约束**(硬):
- 仅本人使用
- WSL 优先(Ubuntu 22.04+),Windows / macOS 不主动适配
- 桌面应用,不做云端部署
- 不包装 Claude Code / Codex SDK
- 数据本地(SQLite 单文件)

---

## 2. 当前进度

**已完成**:
- ✅ 5 份设计文档(README + DESIGN + ARCHITECTURE + TECH + IMPLEMENTATION + BACKLOG)
- ✅ 2 份外部评审(REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro)
- ✅ 8 项设计修订 commit(语义统一、Vue 栈替换、方案 C 留接口 等)
- ✅ 2 个 spike 模板(spike-001 / spike-002,本 session 要跑)

**未开始**:
- ❌ 步骤 0 spike 验证(**当前任务**)
- ❌ MVP 步骤 1-5(等 spike 通过)
- ❌ v1 步骤 6-8

**最近 commit**:
```
b6bdd62 docs: 语义统一 + Vue 栈替换 + 方案 C 留接口
d40ab66 docs: add design review report by deepseek-v4-pro
3562616 docs: add design review report by glm-5.1
327fc1b docs: initial design docs (5 docs files + README, 1841 lines)
```

---

## 3. 文档结构

按"先读这个,再读那个"的顺序:

| 优先级 | 文档 | 什么时候读 |
|--------|------|------------|
| 1 | 本文件(`HANDOFF.md`) | 现在 |
| 2 | `spikes/001-wsl-tauri-window.md` | 跑 spike 前 |
| 3 | `spikes/002-reqwest-anthropic-sse.md` | 跑 spike 前 |
| 4 | `DESIGN.md` | 想了解"为什么这么做"时 |
| 5 | `ARCHITECTURE.md` | 想了解"系统怎么搭"时(尤其 §2 16 关卡) |
| 6 | `TECH.md` | 选库 / 排查技术细节时 |
| 7 | `IMPLEMENTATION.md` | 想知道"下一步做什么"时 |
| 8 | `BACKLOG.md` | 评估新功能时 |
| 9 | `REVIEW-glm-5.1.md` + `REVIEW-deepseek-v4-pro.md` | 想了解"评审怎么说"时(可选) |

**目录**:
```
docs/
├── README.md              # 文档索引
├── DESIGN.md              # 需求 + 边界(明确不做什么)
├── ARCHITECTURE.md        # 16 关卡 + Channel Adapter + 关键决策
├── TECH.md                # 锁定的库(7 项 Vue 栈 + Rust 核心)
├── IMPLEMENTATION.md      # 8 步路线图 + 决策日志
├── BACKLOG.md             # 7 个候选功能 + §9 跨设备(v2)
├── REVIEW-glm-5.1.md      # 外部评审 #1
├── REVIEW-deepseek-v4-pro.md  # 外部评审 #2
├── HANDOFF.md             # 本文件
└── spikes/
    ├── 001-wsl-tauri-window.md
    └── 002-reqwest-anthropic-sse.md
```

---

## 4. 关键决策摘要(8 条)

1. **WSL 优先** — Tauri 跑在 WSL 内,通过 WSLg 显示到 Windows 桌面,无 wslapi 调用
2. **自研 agent core** — 不用 Claude Code / Codex SDK 包装(学习价值 + 控制粒度)
3. **每个 session 一个 git worktree** — 路径统一 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`(为 v2 跨设备做铺垫)
4. **Agent Daemon 化** — agent core 拆出独立进程(v1 之后,飞书或长跑任务痛就拆)
5. **MCP 只外暴露,内部通信不绕** — agent 调自己的工具直接调 Rust 函数
6. **SQLite 是唯一存储** — sqlx + SQLite,FTS5 用于历史搜索
7. **前端栈锁定 Vue 3 + Vite + Pinia + reka-ui** — 不用 React(本 session 才改的)
8. **方案 C:VPS 自托管 daemon(v2 再说)** — 前期只留接口(Channel 协议 network-ready + worktree 跨机器一致)

完整决策日志:[IMPLEMENTATION §4](./IMPLEMENTATION.md#4-决策日志)

---

## 5. 当前任务清单(5 项,已建 TaskList)

按依赖顺序:

1. **[新 session] 跑 spike-001 WSL+Tauri 窗口** — 验证平台层
2. **[新 session] 跑 spike-002 reqwest+Anthropic SSE** — 验证 LLM 链路(可与 #1 并行)
3. **[新 session] 回填 spike 结果 + commit** — 把结果写回 spike 文档
4. **[新 session] 决定后续路径(开始/回退/重评)** — spike 失败时按退路走
5. **[新 session] 后续 spike-003 git2-rs / spike-004 sqlx** — spike-001/002 通过后,跟 MVP 并行

详细执行见各 spike 文档。

---

## 6. spike 怎么跑(高层流程)

**spike-001 / spike-002 可以并行跑**——互不依赖。

### 6.1 spike-001(WSL + Tauri 窗口)
- 在 WSL 终端跑(WSL 2,Ubuntu 22.04 / 24.04)
- 涉及窗口观察,在 Windows 桌面看效果
- **预估 1-3 小时**(含环境准备)
- **硬标准**:窗口显示 + 中文/Emoji 正常 + 10 次热重载不崩 + WebView 进程在 WSL 内
- **硬失败**:走退路 1(XWayland/字体)→ 退路 2(换平台)→ 退路 3(Tauri→Electron)

### 6.2 spike-002(reqwest + Anthropic SSE)
- 在 WSL 或任意 Linux/macOS 跑(纯 CLI,不涉及窗口)
- 需要 `ANTHROPIC_API_KEY` 环境变量
- **预估 30-60 分钟**
- **通过标准**:能 stream + 4 个错误分类正确
- **软退路**:换 rig-core 替手写 / 切 Anthropic 兼容服务

### 6.3 跑完贴回

**spike-001 贴**:
- 启动时间数字
- 中文 / Emoji 渲染现象(可文字描述)
- 10 次热重载成功/失败次数
- `ps aux | grep -iE 'webkit|webview|tauri'` 输出
- 如果失败:完整失败现象 + 已尝试的回退

**spike-002 贴**:
- 成功用例的完整 stdout 输出
- 4 个错误用例的 HTTP 状态码 + 错误响应 body
- 如果失败:失败现象 + 错误信息

### 6.4 回填文档(本 session)

拿到结果后,把内容填到:
- `spikes/001-wsl-tauri-window.md` 的"实际执行 / 结论 / 后续动作"部分
- `spikes/002-reqwest-anthropic-sse.md` 同上
- 改 `**状态**`:待执行 → 通过 / 失败-回退 / 失败-终止
- 改 `**日期**`
- commit 一次(把两个 spike 的结果合并 commit,或分两次)

### 6.5 决定后续路径

| spike 结果 | 后续动作 |
|------------|----------|
| spike-001 通过 + spike-002 通过 | 开始 MVP 步骤 1(搭 Tauri 2 + Vue 3 + Vite 骨架),继续 spike-003/004 并行 |
| spike-001 失败-回退 1 | 试 1-2 天 XWayland / 字体方案,通了就进步骤 1,不通走回退 2 |
| spike-001 失败-回退 2 | 评估换 macOS / Linux 原生,更新 DESIGN §4 |
| spike-001 失败-回退 3 | 评估 Tauri → Electron,更新 TECH §1.3 |
| spike-001 失败-终止 | 重新评估整个项目,WSL + Tauri 是基础平台,这一层不行所有上层设计都失去意义 |
| spike-002 失败 | 软退路,直接用 rig-core 替手写,不阻塞 MVP |

---

## 7. 重要提示(给新 session)

- **不要扩散 scope** — spike 只是验证"能不能跑",不写应用代码
- **不要省略失败回退** — 失败时贴完整现象,不要自己脑补"应该差不多通过"
- **不要跳过"中文 + 热重载 + WebView 进程"** — 这三项是 spike-001 的核心,不是装饰
- **不要在 spike 跑完前开始 MVP** — spike 失败回退可能要重选栈,提前开 MVP 浪费时间
- **spike 文档要被未来读到** — 写"实际执行 / 结论"时按"3 个月后的我能看懂"的标准写

---

## 8. 关联上下文

- **项目根**:`/usr/local/code/github/everlasting/`
- **最近 commit hash**:`b6bdd62`
- **当前 branch**:`main`
- **未推送**:`origin/main` 落后 1 个 commit(本 commit 在本地,等 spike 跑完一起推)

---

> 本文档随项目演进更新。任何重大架构变更后,先改这里,再改具体文档。
