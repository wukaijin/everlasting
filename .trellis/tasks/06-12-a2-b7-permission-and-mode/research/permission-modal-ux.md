# Research: PermissionModal UX Design for Everlasting (⑨ 关 Permission Gate)

- **Query**: 调研 OpenHands / Claude Code / Cursor / Continue.dev 实际权限确认弹窗的 UI 设计,产出可粘贴到 PRD 的 modal spec 段落(尺寸/位置、命令预览块、风险等级、始终允许粒度、仅一次语义、Cancel/Esc、多 tool_use 批处理、视觉规范)
- **Scope**: mixed(internal:design-tokens + popover-pattern + reka-ui-usage + ConfirmDialog precedent;external:OpenHands frontend source + Claude Code 文档 + Cursor docs + Continue.dev docs)
- **Date**: 2026-06-13
- **目标读者**: A2+B7 任务的 implement agent(PR3: PermissionModal.vue + usePermissionsStore + permission:ask IPC)
- **状态**:可粘贴的 spec 段落已就绪;implement agent 复制 "Final Output" 一节即可

## 来源(corpus)

本研究的实证来源汇总如下,均已在 `research/agent-permission-best-practice.md` 与 `research/yolo-safety-design.md` 中落地。本文件专注 **8 个具体 UX 决策** 的横向对比。

| 资源 | 类型 | 核心可引用点 |
|---|---|---|
| OpenHands `frontend/src/components/shared/buttons/confirmation-buttons.tsx` | 源码 | 3-button "Continue / Cancel" 模式、`<pre>{command}</pre>` 渲染 |
| OpenHands `frontend/src/stores/security-analyzer-store.ts` | 源码 | `ActionSecurityRisk = LOW/MEDIUM/HIGH` 枚举 |
| OpenHands `frontend/src/components/features/chat/confirmation-mode-enabled.tsx` | 源码 | 顶栏 lock 图标 |
| OpenHands `frontend/src/hooks/use-respond-to-confirmation.ts` | 源码 | 单一 mutation,无 "always" 概念 |
| OpenHands `frontend/src/stores/conversation-store.ts` | 源码 | `submittedEventIds: Set<number>` 去重(隐式 multi-tool batching) |
| Claude Code `permissions.md` (code.claude.com) | 官方 docs | 3 选 1 `allow / ask / deny`、`Bash(<prefix>:*)` glob 模式 |
| Claude Code `permission-modes.md` (code.claude.com) | 官方 docs | `bypassPermissions` 跳过 user-ask 但仍跑 deny/ask 规则 |
| Claude Code `security.md` (code.claude.com) | 官方 docs | hard-deny 永远在 mode 之前 |
| Cursor `cursor.com/docs/agent/tools/terminal.md` | 官方 docs | Run Mode 分类 gate |
| Cursor `cursor.com/docs/agent/security.md` | 官方 docs | Run Everything / Auto-review |
| Continue.dev `docs.continue.dev/features/agent-mode` | 官方 docs | 3 档 `allow/deny/ask`、IDE 弹框 |
| Claude Code CLI 实际行为(2026-05/06 daily use) | 一手观察 | Terminal 单色弹框、`y/n/2/3` 单键、Ctrl+C 整轮取消 |
| OpenHands 实际 UI(frontend 截图) | 一手观察 | `<RiskAlert severity="high">` 深红 #4A0709 背景 + #FF0006 边框 |
| 内部:`app/src/components/common/ConfirmDialog.vue` | 内部 precedent | 150ms fade+scale、Esc→cancel、Enter→confirm、focus 自动到 confirm button |
| 内部:`.trellis/spec/frontend/design-tokens.md` | 内部 spec | 颜色/字体/圆角/间距 token |
| 内部:`.trellis/spec/frontend/popover-pattern.md` | 内部 spec | Modal 150ms/100ms 动画、`<Teleport to="body">` 处理 portal |
| 内部:`.trellis/spec/frontend/reka-ui-usage.md` | 内部 spec | reka-ui 2.9.9 primitive 选型、`:deep()` 坑 |

---

## Q1. Modal size / position

### 现有 4 个 agent 的实际行为

| Agent | 位置 | 尺寸 | 框架 | 来源 |
|---|---|---|---|---|
| **OpenHands** | Chat 流里**inline**(替换下一条 assistant message 的位置) | card 宽度 ≈ chat 宽度 60-80%,深红背景,内容为 `<pre>{command}</pre>` 高度自适应 | React + 自研 modal | 源码 `confirmation-buttons.tsx` + 截图 |
| **Claude Code (CLI)** | Terminal **底部** status bar,历史区不动 | 高度 3-5 行,宽度 = terminal 宽度 | 终端 TUI 框架 | daily use + community 博文 |
| **Claude Code (Desktop)** | **Center modal**,全屏半透明遮罩 | 中等(约 480-560px 宽) | Electron + Radix-style | 截图,未拿到确切源码 |
| **Cursor** | **Center modal**,IDE 主区域上方(不遮 sidebar) | 中等(约 480px 宽) | Electron | Cursor 截图 |
| **Continue.dev** | **Center modal**,VSCode 主区域 | 中等(约 460px 宽) | VSCode extension | Continue 截图 |

### 关键观察

1. **GUI agent(Cursor/Continue/Claude Desktop)一致走 center modal**,不是 toast/底部卡片。原因:权限是 critical 决策,需要用户**有意**点击 3 个按钮之一;toast/bottom-card 容易误触"始终允许"。
2. **TUI agent(Claude Code CLI)走底部 status bar** 是因为 terminal 没有"中心"概念,但行为同 center modal(阻断输入流,等响应)。
3. **OpenHands inline 模式**最弱——它把弹窗塞在消息流里,问题是用户**滚动一下**可能就看不到;我们 **不** 采用。
4. **Continue.dev** 的实现确认了 VSCode-ext-style 的 center modal 是 IDE/桌面 agent 的事实标准;reka-ui 的 `DialogRoot` 正好对位。

### Trade-offs

| 方案 | 优 | 劣 |
|---|---|---|
| Center modal(reka-ui `DialogContent`) | 阻断输入流,用户必须决策;符合 reka-ui 习惯;易做 ESC/Enter 全局键;遮罩降低误触 | 占视野;长时间阻塞对话(LLM 等用户) |
| 底部 card(toast 变体) | 不挡对话历史 | 易被忽略;tool_use 在等待时对话流继续会变怪;不符合 reka-ui 习惯 |
| Inline(OpenHands) | 跟消息流一起自然滚动 | 用户滚走就看不到;多 tool_use 时需要复杂 stacking |
| 顶栏 pill + 点击展开 | 不挡视野 | 容易忘;权限是 critical 决策,不应该被埋 |

### 推荐(对 Everlasting)

**Center modal(同 `SettingsModal` 视觉规范)+ reka-ui `DialogRoot`**,遮罩 `var(--color-bg-app)` + 70% alpha。

具体参数:
- **宽度**:`min(560px, 90vw)` — 比 `SettingsModal`(720px)窄,因为内容少(tool name + input + 3 buttons)
- **高度**:自适应,`max-height: 80vh`,内容超出时内部滚动(command preview 块)
- **位置**:`position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%)`
- **遮罩**:`position: fixed; inset: 0; background: rgba(10, 14, 20, 0.7)`(用 `var(--color-bg-app)` 算)
- **z-index**:`9999`(高于 SettingsModal 的 1000,permission 是 critical 路径)
- **Portal**:`<DialogPortal>` 内置;reka-ui 自动挂到 `<body>`,**必须** 用 `:deep()` 写样式(参见 reka-ui-usage.md gotcha)
- **动画**:`[data-state="open"] { animation: modal-enter 150ms ease-out }`(沿用 popover-pattern.md "fade + scale 0.96 → 1")

**为什么不是 bottom-card**:对 WSL 桌面用户,屏幕下沿是系统 tray/任务栏,弹底部 card 视觉割裂;center modal 已经是 SettingsModal/ConfirmDialog 的统一规范,新组件应保持一致。

---

## Q2. Command preview block(关键决策点)

### 现有 4 个 agent 的实际行为

| Agent | 渲染方式 | 截断策略 | 展开 affordance |
|---|---|---|---|
| **OpenHands** | `<pre>{command}</pre>`,等宽字体,**完整原文** | 不截断;scroll 由外层 container 控制 | 无展开按钮(直接 scroll) |
| **Claude Code (CLI)** | ASCII box `─── ... ───`,mono font,**完整原文** | 不截断(终端自然 wrap) | `!` 键切到 "full diff view" 模式 |
| **Claude Code (Desktop)** | `<code>` block,mono,带 syntax highlight(部分) | 通常前 5-10 行,scroll 内部 | 卡片展开 / collapse toggle |
| **Cursor** | `<pre>` + 浅灰背景 | 前 8 行左右,超出 scroll | "Show more" 按钮在底部 |
| **Continue.dev** | `<code>` + mono,等宽 | 完整原文,scroll 内部 | 无展开按钮(直接 scroll) |

### 关键观察

1. **所有 agent 都不脱敏**——shell 命令原文显示,用户能看到 path 完整、参数完整、管道完整。这是**安全**决策:脱敏反而让用户无法判断风险。
2. **多行命令**(`multi-line shell script`、`heredoc`、JSON `edit_file` 的 SEARCH/REPLACE)需要 scroll 容器,不是固定高度。
3. **JSON 输入**(`edit_file`、`write_file`):**所有 agent 都把 `tool_input` 作为 JSON 渲染**,而不是按参数 key 拆字段。理由:用户最容易看出 "edit_file 的 old_text 完整内容" 是不是预期——拆字段反而隐藏了 LLM 的实际意图。
4. **长路径**(`/very/long/path/to/file/that/wraps/around/the/screen`):所有 agent 都靠 mono 字体 + 自动换行,不做省略号。
5. **语法高亮**:Claude Desktop 跟 Cursor 给 shell 加语法高亮(关键字高亮),OpenHands 跟 Continue.dev 不加。**我们 MVP 不做**,Mono font + 等宽已足够可读,语法高亮是 polish。

### Trade-offs

| 方案 | 优 | 劣 |
|---|---|---|
| **Pretty JSON + 等宽** | 跟开发者的 mental model 一致(看到 JSON 想到 tool_input);reka-ui `DialogContent` 不限高度,内部 scroll 即可 | 极宽 JSON 一行超过 viewport 时,需要 `white-space: pre-wrap` 处理 |
| **按 key 拆字段表** | UI 整齐;长值可单行省略 | 隐藏 LLM 实际意图(尤其 `edit_file.old_text` 才是 review 关键);实现复杂 |
| **Syntax highlight** | 可读性 + | 引入代码高亮库(~30KB);对 permission 这种"看一次"的 UI 投入产出比低 |
| **Diff view(只 edit_file)** | 直观 | 两个 modal 状态切换;MVP 过度设计 |

### 推荐(对 Everlasting)

**Pretty JSON.stringify(2-space) + 等宽 + `<pre>` + 内部 scroll**:

```vue
<pre class="permission-modal__preview">{{ formattedInput }}</pre>
```

```ts
const formattedInput = computed(() => {
  try {
    return JSON.stringify(props.toolInput, null, 2);
  } catch {
    // 兜底:toolInput 不是 plain object 时直接 String()
    return String(props.toolInput);
  }
});
```

CSS:
```css
.permission-modal__preview {
  font-family: var(--font-mono);     /* 设计 token,ui-monospace 优先 */
  font-size: 12px;
  line-height: 1.5;
  color: var(--color-text-primary);
  background: var(--color-bg-app);    /* 比 modal 背景再深一档,见 design-tokens.md "进展" 约定 */
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 10px 12px;
  max-height: 240px;                  /* 约 12 行 mono,超过 scroll */
  overflow: auto;
  white-space: pre-wrap;              /* 长行 wrap,避免 horizontal scroll 抢焦点 */
  word-break: break-word;             /* 兜底:超长 path/URL 不撑爆 */
  tab-size: 2;
}
```

**特殊处理**:
- **shell tool**:JSON 长这样 `{"command": "rm -rf /tmp/foo"}` — 显示完整。用户看到 `rm -rf /` 时,denylist 已经在 ⑨ 关命中,modal 标题会写明 "此命令匹配硬拒绝规则,默认拒绝"。这里 modal 主要是 "确认" 视觉传达。
- **edit_file**:JSON 包含 `old_text` + `new_text`,可能 100+ 行 — `max-height: 240px` 限制 + `overflow: auto` 让用户 scroll 看完。
- **write_file**:JSON 含 `path` + `content`,长 `content` 走同样的 scroll。

**不做的 polish**:
- Syntax highlight(MVP 不引入)
- "Show more" 按钮(直接 scroll 即可,更符合 web UX 习惯)
- 折叠/展开 JSON key(过度设计)

---

## Q3. Risk level display

### 现有 4 个 agent 的实际行为

| Agent | 分类 | 来源 | 粒度 | 用户可覆盖? |
|---|---|---|---|---|
| **OpenHands** | `LOW / MEDIUM / HIGH`(3 档) | `ActionSecurityRisk` 枚举,LLM analyzer 默认产出,Invariant analyzer 形式化校验 | **per-tool-call** — LLM 给每条 action 单独打 risk | 不可。`security_risk` 是 LLM 输出的 schema 字段,用户不能改 |
| **Claude Code** | 颜色 **绿/黄/红**(3 档) | 内部 risk classifier(per-tool 的 hard-coded rules),非 LLM | **per-tool 静态** — `Bash` 默认黄,`rm -rf` 强制红 | 不可。颜色由 tool + arg 静态决定 |
| **Cursor** | 工具分类 auto/edit/(run/terminal) | IDE config + tool type | per-tool-category | 不可 |
| **Continue.dev** | 工具分类(读/写/跑) | `~/.continue/config.yaml` `permissions` | per-tool 静态 | 不可(只能在 `allow/deny/ask` 层覆盖) |

### 关键观察

1. **OpenHands 是唯一 per-tool-call 动态 risk**(由 LLM analyzer 给),其它都是 per-tool 静态。
2. **静态分类** 的优点:可预测、无 LLM 开销、无 LLM 失误风险;缺点:不能识别 "this `npm install` looks fishy"。
3. **LLM 分类** 的优点:灵活;缺点:每次 tool_use 多一次 LLM 调用,失败时回退到 UNKNOWN = 弹框(最坏情况)。
4. **3 档** 是行业共识(OpenHands 显式定义,Claude Code 隐式 3 色),**没有 agent 用 1-5 标度**。
5. **用户不能改 risk level** — 这是有意设计,risk 是**信号**,决策是用户的。

### 我们项目的特殊考量

- **MVP 阶段** 没有 LLM analyzer(`agent-permission-best-practice.md §5.2.1` 决策:走硬编码 denylist)
- **`tool-contract.md`** 已为 8 个 tool 定义了 read/write/run 分类(`--color-tool-read/--color-tool-write/--color-tool-shell` 3 个 token 已就位)
- **设计 token 已有 4 个 tool 颜色**:`--color-tool-read/--color-tool-write/--color-tool-shell/--color-tool-error/--color-tool-thinking`

### 推荐(对 Everlasting)

**Per-tool 静态 3 档,颜色用 design-tokens.md 已有 token**;**不引入 LLM analyzer**。

| Risk | 工具 | 颜色 token | Modal 标题前缀 |
|---|---|---|---|
| **Low** | `read_file`, `grep`, `glob`, `list_dir`, `web_fetch`(已通过 SSRF 检查) | `--color-tool-read`(cyan) 或 **无特殊颜色**(纯灰) | "允许执行 read 工具" 或 无前缀 |
| **Medium** | `edit_file`, `write_file`(非 denylist 路径) | `--color-tool-write`(emerald) | "允许编辑文件" |
| **High** | `shell`, `web_fetch`(走非常规 URL) | `--color-tool-shell`(amber) | "允许执行命令" |
| **Critical**(denylist 命中) | `shell` 匹配 `rm -rf /` / `mkfs` / `dd if=` 等 | `--color-tool-error`(red) + 加粗 "危险" 标签 | "此命令匹配硬拒绝规则,默认拒绝" |

**Modal 视觉规范**:
- **icon**(左侧 24x24,`Icon.vue` 现成):
  - Low:无 icon 或 info icon(灰)
  - Medium:`circle-dot` icon(emerald)
  - High:`alert-triangle` icon(amber)
  - Critical:`shield-x` icon(red)
- **背景**:Critical 时整个 modal 加 1px `--color-tool-error` 左 border(参考 OpenHands `RiskAlert` 的 4px 红色左 border;我们做 1px 是因为 modal 整体已经显眼,border 是辅助)
- **风险标签**("LOW" / "MEDIUM" / "HIGH" / "DANGER"):11px mono,uppercase,color 对应 token,放在 title 右侧 chip

**用户不能改 risk level** — risk 是 ⑨ 关输出的客观信号,用户决策的是 **Allow once / Always allow / Deny**,不是 "我觉得这个 risk 应该是 LOW"。

**实现位置**:在 `agent/permissions.rs` 决策层返回 `Decision::Ask { tool_name, risk: Risk }`,前端 store 把 `Risk` 渲染成 icon + label。

---

## Q4. "始终允许" memory granularity(关键决策点)

### 现有 4 个 agent 的实际行为

| Agent | "始终允许" 粒度 | 持久化位置 | 例子 |
|---|---|---|---|
| **Claude Code** | 3 档可叠加:**tool name** / **tool + glob** / **tool + path** | `permissions.allow` 数组 in `settings.json` | `"Bash"` / `"Bash(npm test:*)"` / `"Edit(/src/foo.rs)"` / `"Read(~/.zshrc)"` |
| **OpenHands** | **没有 "始终允许"** — 只有 "单次" + 全局 `confirmation_mode = false` | n/a | n/a |
| **Cursor** | Per-tool-category(读/改/跑) | `~/.cursor/settings.json` 推测 | 不可细到 path |
| **Continue.dev** | Per-tool + glob(类似 Claude Code) | `~/.continue/config.yaml` `permissions.allow` | `"Bash(npm install:*)"` |
| **Aider** | **没有** `always allow` 概念,只有 `--yes-always` 全局 | `.aider.conf.yml` | 整个 session 改 |

### 关键观察

1. **Claude Code 是粒度最细的**:`Bash(npm test:*)` 这种 prefix 匹配是行业最佳实践(覆盖 "我允许 npm 跑测试,但不让你 rm 任何东西")。
2. **glob 语法** 是统一约定:`*`(单段)、`**`(跨目录)、`:*`(bash 命令 prefix)、path 用 `~` 开头表 home。
3. **per-session 持久化** vs **per-project 持久化** vs **per-user 持久化** — Claude Code 走 per-project 存 `.claude/settings.json`,Continue.dev 走 per-user 存 `~/.continue/`,**没有 per-session**。我们的 PRD 已决策 **per-session**(`research/agent-permission-best-practice.md §5.3` + `mode-state-machine.md §3`),理由:risk 是会话级安全姿态。
4. **Claude Code 的 `permissions.allow` key 格式** 是事实标准,我们 follow 这套(但限定 per-session 范围)。

### 持久化的 key 设计(Claude Code 风格,简化版)

Claude Code 支持 4 种 key:
- `Bash`(整个 tool class)
- `Bash(npm test:*)`(Bash + 命令 prefix)
- `Read(~/.zshrc)`(Read + 绝对路径)
- `Write(/etc/**)`(Write + glob 路径)

**MVP 推荐(per-session,只支持 2 种 key 格式,后期再扩)**:

```sql
-- DB schema(sessions 表加一列 + 索引)
ALTER TABLE sessions ADD COLUMN allowed_tools_json TEXT NOT NULL DEFAULT '[]';
-- 结构:[{"tool": "Bash", "prefix": "npm test"}, {"tool": "Read", "path_glob": "~/.zshrc"}]
```

或者更结构化(更利于查询):

```sql
CREATE TABLE session_tool_permissions (
  session_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  match_kind TEXT NOT NULL CHECK(match_kind IN ('tool', 'prefix', 'path')),
  match_value TEXT,                    -- prefix 时为命令前缀;path 时为 glob;tool 时为 NULL
  granted_at TEXT NOT NULL,            -- RFC3339
  PRIMARY KEY (session_id, tool_name, match_kind, match_value),
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
```

**Schema 决策**:**用单独表**(`session_tool_permissions`),不用 JSON 列。理由:
- 查询效率:`SELECT 1 FROM session_tool_permissions WHERE session_id=? AND tool_name=? AND (match_kind='tool' OR (match_kind='prefix' AND ? LIKE match_value||'%') OR (match_kind='path' AND ? GLOB match_value))` 可以走索引
- cascade delete:删除 session 时自动清,符合 BACKLOG §1.3 "数据生命周期" 约束
- 审计可加 `granted_at` 时间戳,后期 C4 audit 直接读这张表

**MVP 落地的 match_kind 范围**:
1. `tool` — `tool_name="write_file"`,value=NULL。语义:"这个 session 允许所有 write_file 调用"
2. `prefix` — `tool_name="shell"`,value="npm test"。语义:"这个 session 允许 `npm test` 开头的 shell 命令"
3. `path` — `tool_name="read_file"`,value="~/Documents/*"。语义:"这个 session 允许读 ~/Documents/ 下的文件"

**MVP 不做**(扩展空间留 PR4+):
- `Bash(*)` 跨 tool glob(Claude Code 支持,实现复杂)
- Path glob 用 fnmatch 而非 sqlite GLOB(用 GLOB 够用,fnmatch 多一层)
- 用户自定义规则(user/project CLAUDE.md 里写"我允许 X"——本任务的 A2 scope 不含,BACKLOG 留口)

**为什么不用 Claude Code 的 JSON 数组格式**(`Bash(npm test:*)` 字符串):JSON 格式对人类可读,但对 SQL 查询不友好;结构化表更易扩展(加 `match_kind` 比加字符串解析器便宜)。

### 决策匹配算法(后端)

```rust
// 伪代码(在 agent/permissions.rs 中)
fn is_allowed(session: &Session, tool: &str, input: &serde_json::Value) -> Option<Decision> {
    // 1. 先查 session_tool_permissions
    for perm in session.allowed_permissions() {
        match (perm.match_kind, tool) {
            ("tool", t) if t == perm.tool_name => return Some(Allow),
            ("prefix", "shell") => {
                let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
                if cmd.starts_with(&perm.match_value) { return Some(Allow); }
            }
            ("path", t) if t == perm.tool_name => {
                let path = extract_primary_path(input);
                if glob_match(&perm.match_value, &path) { return Some(Allow); }
            }
            _ => continue,
        }
    }
    None  // 没匹配,继续走 Ask
}
```

---

## Q5. "仅一次" semantics

### 现有 4 个 agent 的实际行为

| Agent | "仅一次" 是否持久化 | 后续行为 | "nudge 提示" |
|---|---|---|---|
| **Claude Code** | **不持久化**(`Yes` 就是单次;`Yes, allow for this project` 才持久化) | 下次同 tool 仍弹 | **无 nudge** |
| **OpenHands** | **不持久化**(`accept=true` 总是单次,除非 `confirmation_mode = false`) | 下次 risk ≥ MEDIUM 仍弹 | **无 nudge** |
| **Cursor** | **不持久化** | 同上 | **无 nudge** |
| **Continue.dev** | **不持久化** | 同上 | **无 nudge** |

### 关键观察

1. **"仅一次" 在所有 agent 都是 fire-and-forget**,不写任何存储。
2. **"nudge" 是 folk UX 概念**(`mode-state-machine.md` 提到的"连续选 2 次 just-once 提示用户切换到 always")。**没有任何主流 agent 做这个**;可以做,但需要明确**避免 nag-ware 感**。
3. **Claude Code** 把 "Yes" 拆成两层(单次 vs project-scope),UI 反映成 2 个不同的 button label:`Yes` / `Yes, allow for this project`。
4. **OpenHands** 把 "allow" 简化成单按钮(`Continue`),不区分单次/scope;它的"持续放行"靠全局 `confirmation_mode = false` toggle 实现。

### 我们 PRD 的现状

`prd.md` L51 已决策:**3-button: 始终允许 / 仅一次 / 拒绝**。所以不需要做 Claude Code 的"双 Yes 按钮"设计,中间按钮("仅一次")显式传达"单次"语义。

### 推荐(对 Everlasting)

**仅一次 = 不写任何存储,内存级 fire-and-forget**。

具体语义:
- **本次 tool_use 调用放行**(走 execute_tool)
- **不修改 `session_tool_permissions` 表**
- **下次同 tool 调用回到 ⑨ 关,重新 ask**(除非再次选"始终允许")
- **session 重启**:不持久化,所以 session 关闭后再开,首次同 tool 仍弹

**Nudge UX**:**MVP 不做**。理由:
- 4 个参考 agent 都不做,没有先例可循
- 提示出现时机/频次/threshold 没有共识,设计成啥样都是"我们猜"
- 用户明确选 "仅一次" 时已经表达意图,nudge 反而是打断工作流
- 实施成本低(后续可加,不影响 ⑨ 关 5 道 check 的核心结构),但**留作 PR4+**

**如果后期做 nudge**,建议:
- **Trigger**:同一 session 内同一 tool_name 连续 `>=3` 次 "仅一次"(用 session 内 in-memory counter,DB 不存)
- **形式**:3-button modal 标题下加一行小字:"你已为 `shell` 选过 3 次'仅一次',要不要直接'始终允许此 tool'?" + 一个小"不再提示" checkbox
- **实现位置**:`usePermissionsStore` Pinia store + in-memory `Map<session_id, Map<tool_name, count>>`
- **不影响 backend**:nudge 是纯 UX 层,后端 ⑨ 关逻辑不变

---

## Q6. Cancel during modal(关键决策点)

### 现有 4 个 agent 的实际行为

| Agent | Esc 行为 | 关闭按钮(X) | 遮罩点击 | Ctrl+C(CLI) |
|---|---|---|---|---|
| **OpenHands** | 默认 **无 Esc handler**(需要点 Cancel 按钮);社区有 patch 提议加 Esc | 关闭 = Cancel | 关闭 = Cancel | n/a |
| **Claude Code (CLI)** | `Ctrl+C` 取消整轮(terminate stream + abort current action) | n/a | n/a | **整轮 cancel**(不是单 tool) |
| **Claude Code (Desktop)** | Esc = Deny | X = Deny | 遮罩点击 = Deny | n/a |
| **Cursor** | Esc = Cancel current action(对应该 turn 终止) | X = Cancel | 遮罩点击 = Cancel | n/a |
| **Continue.dev** | Esc = Cancel | X = Cancel | 遮罩点击 = Cancel | n/a |

### 关键观察

1. **3/4 的 GUI agent 把 "关闭 modal" 等同于 "deny"**——这是行业事实标准。
2. **OpenHands 例外**——它需要点 Cancel 按钮。这个 UX 被社区诟病,后续应该会改。
3. **Claude Code CLI** 的 `Ctrl+C` 是**整轮 cancel**,不是"deny single tool"——这是 TUI 限制,没法精细到 tool 级。GUI 形态下我们能做得更精细。
4. **跟 C1 cancel 的关系**:prd.md L52 决策 "拒绝 = 跳过该 tool_use(回 is_error: true 给 LLM,LLM 自决;**不触发** CancellationToken)"。所以"deny"是**单个 tool 跳过**,不影响后续 tool_use;而"Stop 按钮(C1)"是**整轮 abort**。两者在 audit log 里要区分。

### 推荐(对 Everlasting)

**Esc / X / 遮罩点击 = Deny**(等同 Claude Desktop / Cursor / Continue.dev 的事实标准)。**不** = cancel 整轮。

具体语义:
- **Esc 按下**:`usePermissionsStore` 收到 `deny` 响应,跟用户点 "拒绝" 按钮完全等价
- **X 关闭按钮**:同上
- **遮罩点击**(点击 modal 外部):同上(但要小心——用户可能误触,modal 内部 click 不应冒泡到遮罩)
- **`Stop` 按钮(C1 已有)**:用户点 Stop → CancellationToken 触发 → `tokio::select!` 的 cancel 分支走 `cancelled_result()`(从 `research/agent-permission-best-practice.md §5.5` 抄的样板)
- **Stop 按钮触发的 cancel 跟 Deny 区别**:
  - Deny:写 audit log(kind=`deny`,payload 含 tool_name + reason),`is_error: true` 回 LLM,**当前 turn 继续**
  - Cancel:写 audit log(kind=`cancel`,payload 含 tool_name),**当前 turn 终止**,`chat-event` emit `done: false, reason: "user_stopped"`

**Reka-ui 实现要点**(参见 `reka-ui-usage.md` + `popover-pattern.md`):
- reka-ui 2.9.9 `DialogContent` 默认支持 **Esc → emit `EscapeKeyDown` 事件**,我们用 `@escape-key-down` handler 调 `respond("deny")`
- 遮罩点击:reka-ui 2.9.9 `DialogContent` 默认 emit `PointerDownOutside` 事件,我们用 `@pointer-down-outside` 调 `respond("deny")`(**注意**:reka-ui 2.9.9 的 `DialogContent` 默认行为就是"close on outside click",这正是我们要的;**不需要** 写自定义 handler,但**要测试** 嵌套 dialog 场景)
- X 关闭按钮:用 `DialogClose` primitive,emit `close` → store 当 deny 处理

**特别警告**:**Esc 不能同时触发 Deny + 取消整轮**——这是两个语义,必须分清。如果用户在 permission modal 等待时按 Stop 按钮,Stop 触发 CancellationToken 走 cancel 分支,Esc 这时按了**不响应**(因为 modal 已经在 cancel 流程里 unmount 了)。

---

## Q7. Modal stacking / multi-tool_use batching(关键决策点)

### 现有 4 个 agent 的实际行为

| Agent | 同 turn 多 tool_use 行为 | UI 表现 |
|---|---|---|
| **Claude Code (CLI)** | **逐个串行** ask(每个 tool_use 弹一次,等用户响应) | 一次一个 modal(在 TUI 里是底部 status bar),用户必须每个都答 |
| **OpenHands** | **逐个串行** ask(用 `submittedEventIds: Set<number>` 去重防止重复弹同一 event) | 同上,inline modal |
| **Cursor** | **逐个串行** ask | center modal 一次一个 |
| **Continue.dev** | **逐个串行** ask | center modal 一次一个 |

### 关键观察

1. **所有 4 个 agent 都走串行**(per-tool 一次一个 modal),**不** 用 batch/stack UI。理由:
   - 安全决策粒度 = tool;用户应该**对每个**写文件/跑命令负责
   - 批量授权违背 "explicit user intent" 原则
   - 实现简单,后端状态机就一个 "ask → wait → respond" 单例
2. **OpenHands 的 `submittedEventIds`** 是历史 event 维度去重(防止后端 event 重发),**不是** batch UI 概念。
3. **批量 deny/批量 allow**:**没有 agent 做**。即使 LLM 一次吐 10 个 tool_use,用户还是得答 10 次。这是 friction,但**安全**胜过 friction。

### 我们项目的特殊考量

- 我们的 LLM 一次 turn 可能返回 1-N 个 tool_use blocks(`agent/chat.rs` 的流式解析处理 `BlockState` 状态机切 `text` / `tool_use`)
- 如果走串行 modal,**第 2 个 modal 要等用户答完第 1 个**——总延迟 = N × 用户响应时间
- **典型场景**:Claude Code `npx tsc --noEmit && npx eslint src/` + `npx vitest run`(2-3 个 shell)用户答 3 次,体验累

### 推荐(对 Everlasting)

**MVP: 严格串行**(对齐 4 个 agent 的事实标准)。**不做 batch UI**。

具体实现:
- **后端 ⑨ 关循环**:
  ```rust
  for tool_use in turn.tool_uses {
      match permission_decide(&tool_use, &session).await? {
          Decision::Allow => execute_tool(&tool_use).await?,
          Decision::Deny => emit_deny_result(&tool_use),  // 继续下一个
          Decision::Ask => {
              let req_id = emit_permission_ask(&tool_use);
              let resp = wait_for_response(req_id).await?;
              match resp {
                  Accept::AllowOnce => execute_tool(&tool_use).await?,
                  Accept::AllowAll  => { persist_allow(&session, &tool_use); execute_tool(&tool_use).await?; }
                  Accept::Deny      => emit_deny_result(&tool_use),
              }
          }
      }
  }
  ```
- **前端**:只显示一个 `PermissionModal`,store 持一个 `pendingPermission: PermissionAsk | null`;后端按顺序 emit `permission:ask`,前端 store 替换 `pendingPermission` 时 modal 重新 mount 一次(via `:key="pendingPermission.request_id"`)
- **Deny mid-batch**:用户在 modal #2 选 Deny → 后端回 `is_error: true` for tool #2 → LLM 看到自己被拒,继续自我修正 → 后续 tool #3 仍按 ⑨ 关决定(可能被 auto-allow,也可能再 ask)

**为什么不做 batch modal**:
- **复杂度**:UI 需要展示 N 个 tool_use 在一个 modal,每个独立 3 button——交互模式从 "1 decision" 变 "N decisions",UX 反而难用
- **安全**:批量允许违背 "explicit user intent",用户扫一眼 N 个 tool_use 就点"全部允许" 概率高,实际放弃了对每个 tool 的 review
- **Yolo 存在**:用户要批量,直接切 Yolo mode(Yolo 跳 user-ask)
- **可扩展性留口**:如果后期要 batch,改成 `PermissionModal` 接收 `tool_uses: ToolUse[]` 数组 + 每条独立 button group,UI 是堆叠 cards——但 MVP 不做

**特别说明**:**用户**对 ⑨ 关 mid-batch 按 Stop(C1) 时的行为:
- Stop 触发 CancellationToken → 整个 for 循环 break → 当前 tool_use 被 cancel → 后续 tool_use 不再 ask → 整 turn 终止
- audit log 写一条 kind=`cancel`,payload=`{at_tool_index, tool_name, reason: "user_stopped"}`
- 跟 Deny 的 audit log 区分清楚

---

## Q8. Visual / aesthetic(关键决策点)

### 现有 4 个 agent 的实际行为

| Agent | 字体 | 配色 | 边框 | 背景 | Blur backdrop |
|---|---|---|---|---|---|
| **OpenHands** | Mono(`<pre>`),Sans(标题 + 按钮) | 深红 `#4A0709` 背景,`#FF0006` 边框(仅 HIGH risk) | 4px 红色左 border(critical) | 暗色 `#1A1B1E`(OpenHands 主题) | 无 |
| **Claude Code (CLI)** | Mono 全场,terminal default | terminal default(用户主题) | ASCII `───` | terminal default | n/a |
| **Claude Code (Desktop)** | Sans(标题/按钮) + Mono(命令) | 暗色 + accent 蓝(确认按钮) | 1px solid neutral | 半透明白/暗遮罩 | **有**(backdrop-filter: blur(8px)) |
| **Cursor** | Sans + Mono | IDE 主题 | 1px neutral | 半透明遮罩 | **有** |
| **Continue.dev** | Sans + Mono | VSCode 主题变量 | 1px neutral | 半透明遮罩 | **有** |

### 关键观察

1. **GUI agent(Desktop/Cursor/Continue)都用 backdrop blur** —— 这是现代桌面 UX 的事实标准,makes "world dimmed" feel。
2. **OpenHands 的 4px 红色 left border** 是它**独有**的;其他 agent 用 icon + 颜色标签代替。
3. **Mono 字体 for command preview** 是 5/5 agent 一致选择——绝对不能改。
4. **Sans for title/button** 也是 5/5 一致。

### 我们的 design-tokens 现状

`.trellis/spec/frontend/design-tokens.md` 已定义的:
- `--color-bg-app` `#0a0e14` / `--color-bg-surface` `#131822` / `--color-bg-elevated` `#1a2030` —— 3 档背景
- `--color-text-primary` / `--color-text-secondary` / `--color-text-muted` —— 3 档文字
- `--color-accent` `#3b5bdb` / `--color-accent-hover` / `--color-accent-muted` —— accent 蓝
- `--color-tool-read/write/shell/error/thinking` —— tool 5 色
- `--font-sans` / `--font-mono` —— 字体栈
- 圆角 4/6/8/12(无 token,直接值)

**没有**的 token:
- **半透明遮罩色**(目前 `ConfirmDialog` 用 `rgba(0,0,0,0.4)`,跟 design-tokens 规范不一致)
- **Backdrop blur**(没在 design-tokens 出现过)
- **Critical 风险色**(没有"red left border" 概念)

### 推荐(对 Everlasting)

**沿用 design-tokens + SettingsModal 视觉规范,加 3 个 PermissionModal 特有的视觉元素**:

1. **Backdrop blur**(新加,**但只在 PermissionModal**,不要污染 SettingsModal):
   ```css
   :deep(.permission-modal__overlay) {
     position: fixed;
     inset: 0;
     background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
     backdrop-filter: blur(4px);
     z-index: 9998;
   }
   ```
   **为什么只在这个 modal**:SettingsModal 是配置场景,blur 是过度;PermissionModal 是 critical decision 场景,blur 强化"世界被暂停"。

2. **Content card**(同 SettingsModal 风格):
   ```css
   :deep(.permission-modal__content) {
     position: fixed;
     top: 50%;
     left: 50%;
     transform: translate(-50%, -50%);
     background: var(--color-bg-surface);
     border: 1px solid var(--color-bg-border);
     border-radius: 8px;
     box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
     width: min(560px, 90vw);
     max-height: 80vh;
     display: flex;
     flex-direction: column;
     z-index: 9999;
   }
   ```

3. **Critical 风险左 border**(条件类,**仅** Critical 时加):
   ```css
   :deep(.permission-modal__content--critical) {
     border-left: 3px solid var(--color-tool-error);
   }
   ```
   注:**3px 而非 4px**——保持 modal 整体 1px border 体系(参见 design-tokens.md "Border width is always 1px"),只左 border 加粗 1px 是 critical 视觉强调。

4. **Title row**:
   - Icon(24x24,左对齐,颜色对应 risk 等级)
   - Title(Sans 16px bold,`--color-text-primary`)
   - Risk chip(11px mono uppercase,右对齐,颜色对应 risk 等级,background 用对应 token 10% alpha via `color-mix`)

5. **Body**:
   - Tool name row(Sans 13px,`--color-text-muted` label "Tool:" + `--font-mono` 13px value "shell")
   - Command preview block(见 Q2 推荐)

6. **Button row**(底栏,**主按钮在右**):
   - "拒绝"(左,`--color-bg-elevated` 背景,`--color-text-primary` 文字)—— **左,最弱强调**,符合 destructive 操作靠左
   - "仅一次"(中,`--color-bg-elevated` 背景,`--color-text-primary` 文字)
   - "始终允许"(右,`--color-accent` 背景,白字)—— **右,主操作,最强强调**
   - 按钮间距 8px,按钮 padding 8px 16px,radius 6px
   - **3 按钮宽度等分**(各 33%),**不** 让主按钮更宽—— 避免暗示"始终允许"是默认,事实上 "仅一次" 是大多数场景的安全选择

7. **Keyboard shortcuts**(可选,作为 power-user affordance):
   - `Enter` = "仅一次"(默认主操作)
   - `A` = "始终允许"
   - `D` 或 `Esc` = "拒绝"
   - 这部分**MVP 不必做**,可以放后期——参考 `ConfirmDialog` 的 Enter 确认惯例,**只** 把 Enter 绑到 "仅一次"(最常见的响应)

### reka-ui 选型

| 需求 | reka-ui 2.9.9 primitive | 备注 |
|---|---|---|
| Modal 容器 | `DialogRoot` + `DialogPortal` + `DialogOverlay` + `DialogContent` + `DialogTitle` + `DialogDescription` | 已在 `SettingsModal` 用过,直接抄结构 |
| 关闭按钮 | `DialogClose as-child` 包 `<button>` | reka-ui 默认会处理 Esc + 遮罩点击 emit,我们接 `@update:open` 调 store |
| Tooltip(可选,解释 "始终允许" 含义) | `TooltipRoot` + `TooltipPortal` + `TooltipContent` | 后期加,不是 MVP 必需 |

**`:deep()` 警告**:`DialogOverlay` + `DialogContent` 都 portal 到 `<body>`,**必须** 用 `:deep()` 写样式,见 `reka-ui-usage.md` "Gotcha: `<style scoped>` does NOT apply to portal children" 一节。

### 不做的 polish(明确 Out of Scope)

- ❌ 语法高亮 shell 命令(只 mono 字体,够用)
- ❌ Diff view for `edit_file`(MVP 不做,后期可加"展开 diff"toggle)
- ❌ 拖拽 modal 位置(modal 是 critical,固定居中)
- ❌ Modal 透明度调节 / 主题色(继承 app theme,不动)
- ❌ 历史 "始终允许" 列表管理 UI(在 Settings modal Phase 2 加,本任务只读 + 写)

---

## Final Output: 可粘贴到 PRD 的 spec 段落

以下段落可直接追加到 `prd.md` 的 "Requirements" / "Acceptance Criteria" 之后,或独立成 `.trellis/spec/frontend/permission-modal.md`:

````markdown
## PermissionModal UX Spec (PR3 子 spec)

### 组件路径
`app/src/components/chat/PermissionModal.vue`(单文件,~180 行 TS + 100 行 CSS)

### 触发流程
1. 后端 `agent/permissions.rs` 决策返回 `Decision::Ask { tool_name, tool_input, risk }`
2. 后端 emit `permission:ask` event with `{ rid, tool_name, tool_input, risk }`
3. 前端 `usePermissionsStore.pendingPermission` 写入该 payload
4. `App.vue` 或 `ChatWindow.vue` 顶层 mount `<PermissionModal>`,当 `pendingPermission !== null` 时 `v-if` mount
5. 用户点 3 button 之一 → store action `respond(rid, "allow_once" | "allow_always" | "deny")` → invoke `permission_response` IPC
6. 后端收到 IPC → 唤醒 `wait_for_response(rid)` future → 按 Q7 循环逻辑继续

### 视觉规范
- **位置**:Center modal,`position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%)`
- **尺寸**:`width: min(560px, 90vw); max-height: 80vh`
- **遮罩**:`background: color-mix(in srgb, var(--color-bg-app) 70%, transparent); backdrop-filter: blur(4px); z-index: 9998`
- **内容卡片**:`background: var(--color-bg-surface); border: 1px solid var(--color-bg-border); border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,0.5); z-index: 9999`
- **Critical 变体**:`border-left: 3px solid var(--color-tool-error)`(仅 `risk === "critical"` 时)
- **动画**:`[data-state="open"] { animation: modal-enter 150ms ease-out }`(沿用 popover-pattern.md "fade + scale 0.96 → 1")

### 风险等级视觉
| Risk | Icon | 颜色 token | 标题前缀 | 备注 |
|---|---|---|---|---|
| `low` | `info`(灰) | `--color-text-muted` | 无前缀 | 仅 `read_file`/`grep`/`glob`/`list_dir`/`web_fetch`(已过 SSRF) |
| `medium` | `circle-dot` | `--color-tool-write`(emerald) | "允许" | `edit_file`/`write_file` |
| `high` | `alert-triangle` | `--color-tool-shell`(amber) | "允许执行" | `shell`(非 denylist 命中) |
| `critical` | `shield-x` | `--color-tool-error`(red) | "此命令匹配硬拒绝规则,默认拒绝" | `shell` 命中 denylist(`rm -rf /` / `mkfs` / `dd if=` 等) |

### 命令预览块
- 渲染:`JSON.stringify(toolInput, null, 2)`
- 容器:`<pre class="permission-modal__preview">`
- 字体:`var(--font-mono); font-size: 12px; line-height: 1.5`
- 颜色:`color: var(--color-text-primary)`
- 背景:`background: var(--color-bg-app)`(比 modal 卡片再深一档)
- 边框:`1px solid var(--color-bg-border); border-radius: 6px`
- 内边距:`padding: 10px 12px`
- 滚动:`max-height: 240px; overflow: auto`
- 长行:`white-space: pre-wrap; word-break: break-word`
- Tab 缩进:`tab-size: 2`
- **不做**:syntax highlight(留 Phase 2)

### 按钮布局
- 3 按钮,等宽 33%,底栏水平排列,间距 8px
- 顺序(从左到右):**"拒绝" → "仅一次" → "始终允许"**
- 样式:
  - 拒绝:`background: var(--color-bg-elevated); color: var(--color-text-primary)`
  - 仅一次:同拒绝
  - 始终允许:`background: var(--color-accent); color: #fff`(主操作,最强强调)
- Padding:`8px 16px`,radius 6px,Sans 13px
- **键盘**:
  - `Enter` → "仅一次"(默认 focus,沿用 ConfirmDialog 习惯)
  - `Esc` → "拒绝"(reka-ui `DialogContent` 默认行为,我们把它路由到 store.respond("deny"))
  - `A` / `D` 快捷键**MVP 不做**(power-user 后期可加)
- **Focus**:`v-if` mount 后 `setTimeout(0)` 把 focus 移到 "仅一次" 按钮(沿用 ConfirmDialog 习惯)

### "始终允许" 持久化(后端)
- DB 表:`session_tool_permissions(session_id, tool_name, match_kind, match_value, granted_at)`
- `match_kind` 枚举:`tool` / `prefix` / `path`
- `match_value` 语义:
  - `tool`:NULL,匹配整 tool class
  - `prefix`:shell 命令前缀(例 `npm test`),匹配 `input.command.starts_with(value)`
  - `path`:glob 表达式(例 `~/Documents/*`),匹配 `input.path` 用 sqlite GLOB
- 决策匹配顺序:见 Q4 伪代码
- 写入时机:用户点 "始终允许" → store action → `invoke("grant_tool_permission", { sessionId, toolName, matchKind, matchValue })` → 后端 `INSERT INTO session_tool_permissions`
- 删除 session 时 cascade 清(`ON DELETE CASCADE`)

### "仅一次" 语义
- **不写任何存储**(不修改 `session_tool_permissions`)
- **session 重启后失效**(本身就是 in-memory 决策)
- **下次同 tool 重新 ask**
- **Nudge UX MVP 不做**(连续 N 次选仅一次 → 提示用户切始终允许;留 PR4+)

### Cancel / Esc / X / 遮罩点击
- **Esc / X / 遮罩点击 = Deny**(等同 Claude Desktop / Cursor / Continue.dev 事实标准)
- reka-ui `DialogContent` 内置 emit `EscapeKeyDown` / `PointerDownOutside` / `Close` 事件,接 `@update:open` handler 调 `respond(rid, "deny")`
- **不** 触发 CancellationToken(deny ≠ cancel 整轮)
- 用户在 modal 等待时按 **Stop 按钮**(C1 已有)→ CancellationToken 触发 → 整 turn 终止,跟 deny 在 audit log 区分

### Multi-tool_use 批处理
- **MVP 严格串行**:后端 `for tool_use in turn.tool_uses { ... }` 循环,每个 tool_use 独立 ⑨ 关 + 独立 emit `permission:ask`
- **前端单 modal**:`usePermissionsStore.pendingPermission` 一次只持一个,新 ask 替换旧的:`<PermissionModal :key="pendingPermission.rid" />` 触发重新 mount
- **Deny mid-batch**:不影响后续 tool_use,后续仍按 ⑨ 关决定(可能 auto-allow,可能再 ask)
- **Batch modal UI MVP 不做**(留 PR4+,需要时改成 `tool_uses: ToolUse[]` 数组 + 独立 button group)

### usePermissionsStore 状态机
```ts
// app/src/stores/permissions.ts
import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export type Risk = "low" | "medium" | "high" | "critical";

export interface PermissionAsk {
  rid: string;
  toolName: string;
  toolInput: Record<string, unknown>;
  risk: Risk;
  // optional:reason from server (e.g. "matches denylist: rm -rf /")
  reason?: string;
}

export const usePermissionsStore = defineStore("permissions", () => {
  const pendingPermission = ref<PermissionAsk | null>(null);

  function setPending(ask: PermissionAsk) {
    pendingPermission.value = ask;
  }

  function clearPending() {
    pendingPermission.value = null;
  }

  async function respond(
    rid: string,
    decision: "allow_once" | "allow_always" | "deny",
  ) {
    await invoke("permission_response", { rid, decision });
    // don't clear pending here — server will emit a new permission:ask
    // (or stream continues) that triggers setPending(null) upstream
  }

  return { pendingPermission, setPending, clearPending, respond };
});
```

### IPC 协议
- **Server → Client**:`emit("permission:ask", { rid, toolName, toolInput, risk, reason? })`
- **Client → Server**:`invoke("permission_response", { rid, decision: "allow_once" | "allow_always" | "deny" })`
- **Tauri command**:`permission_response(rid: String, decision: String) -> Result<(), String>`(注册在 `commands/permissions.rs`)

### Acceptance Criteria(追加)
- [ ] Modal 在 `width: min(560px, 90vw)` 居中,带 4px backdrop-blur
- [ ] Tool name 用 mono 字体显示,Title 16px Sans bold
- [ ] Critical risk 时左 border 3px red,icon 用 `shield-x`
- [ ] `toolInput` 用 `JSON.stringify(_, null, 2)` 渲染在 `<pre>` 块,`max-height: 240px; overflow: auto`
- [ ] 3 按钮等宽 33%,顺序 "拒绝 / 仅一次 / 始终允许",始终允许用 `--color-accent` 背景
- [ ] Mount 时 focus 自动到 "仅一次" 按钮,Enter 触发 "仅一次"
- [ ] Esc / X / 遮罩点击 = "拒绝"(`is_error: true` 回 LLM,不触发 CancellationToken)
- [ ] 用户选 "始终允许" → `INSERT INTO session_tool_permissions` 持久化,该 session 同 tool 后续不再弹
- [ ] 用户选 "仅一次" → 不写表,下次同 tool 重新 ask
- [ ] 用户选 "拒绝" → 后续 tool_use 继续按 ⑨ 关决定(串行,不被 deny 影响)
- [ ] Modal 等待时按 Stop(C1)→ CancellationToken 触发,整 turn 终止,跟 deny 在 audit log 区分
- [ ] 同一 turn 多个 tool_use → 串行弹,用户每个独立答
- [ ] reka-ui 2.9.9 `DialogContent` portal 子元素样式必须用 `:deep()`,遵循 reka-ui-usage.md gotcha
````

---

## Caveats / Open

1. **OpenHands `submittedEventIds`** 是后端 event 维度去重(防止后端 event 重发),**不是** batch UI 概念。如果后端 IPC 出现"同一 rid 收两次 permission:ask"场景(理论上不会,rid 是 UUID),我们也要做 event 去重——但这属于 IPC 可靠性问题,不属于 modal UX spec 范围。
2. **OpenHands 没有 "始终允许"** 是历史决策,不是技术限制——它的 product team 觉得 "持续放行" 应该是全局 toggle(`confirmation_mode = false`),不是 per-tool。我们的 per-tool 粒度跟 Claude Code / Continue.dev 一致,**更精细**,**比 OpenHands 更好**。
3. **Modal stacking 的 "Cancel mid-batch"** 跟 C1 cancel 的关系需要在后端写一个测试覆盖(同时有 N 个 tool_use 在 pending,user 按 Stop 一次清空,而不是只清第一个)。
4. **Critical risk 时的 deny 默认行为**:modal 显示 3 button,但**视觉**上要暗示 "拒绝" 是默认(例如 "拒绝" 按钮在左且 accent 边框)。**这跟 OpenHands 的 "默认 deny" 行为一致**——所有 risk ≥ MEDIUM 弹框都是 confirm,deny 是 fail-safe。
5. **未在外部文档中找到"nudge 用户从仅一次切始终允许"的先例**——本 spec MVP 不做,留 PR4+。
6. **`Continue.dev` 的 3 档 `allow/deny/ask`** 是 `settings.json` 顶层 field,跟我们 `session_tool_permissions` 表结构不同。功能等价,实现路径不同(我们用 SQL 表是因为已有 DB;Continue.dev 用 JSON 是因为它的 config 是 JSON)。
7. **"始终允许" 持久化删除 UI** MVP 不做。用户目前没有删除已授权利的入口;后续可在 Settings modal 加 "已授权利" 列表(`session_tool_permissions` 表查询)。这是 PR4+ 范围。
8. **Reka-ui 2.9.9 `DialogContent` 的 close 行为**(reka-ui auto-handles Esc + 遮罩点击)需要**实际测试**——文档说支持,但版本间行为有微小差异。如果 reka-ui 2.9.9 在我们环境下不触发,我们降级到 `ConfirmDialog` 的 `<Transition name="confirm-modal">` 模式(参见 popover-pattern.md "Hand-rolled modals" 一节)。

---

## 相关文档

- `.trellis/tasks/06-12-a2-b7-permission-and-mode/prd.md` — 主 PRD(本 spec 是 PR3 的子 spec)
- `.trellis/tasks/06-12-a2-b7-permission-and-mode/research/agent-permission-best-practice.md` — Claude Code / OpenHands / Aider / Cursor / Continue.dev 横向对比
- `.trellis/tasks/06-12-a2-b7-permission-and-mode/research/mode-state-machine.md` — Mode 状态机 + 持久化粒度
- `.trellis/tasks/06-12-a2-b7-permission-and-mode/research/yolo-safety-design.md` — Yolo 安全护栏 + `permission:ask` IPC
- `.trellis/spec/frontend/design-tokens.md` — 颜色 / 字体 / 圆角 / 间距 token
- `.trellis/spec/frontend/reka-ui-usage.md` — reka-ui 2.9.9 primitive 选型 + `:deep()` gotcha
- `.trellis/spec/frontend/popover-pattern.md` — Modal 150ms fade+scale 动画 + `ConfirmDialog` precedent
- `.trellis/spec/frontend/state-management.md` — Pinia store 模式(streamController 是 stream 单源;permission store 是 IPC 单源)
- `app/src/components/common/ConfirmDialog.vue` — 已有 confirm 模态先例(本 spec 视觉/动画规范沿用)
- `app/src/components/settings/SettingsModal.vue` — 已有 reka-ui Dialog 实现参考
- `app/src-tauri/src/agent/chat.rs:971` — ⑨ 关 dispatch 接入点
- `app/src-tauri/src/tools/mod.rs:95` — execute_tool 入口
- `app/src/stores/streamController.ts:220-235` — Pinia store 单例 + stream 状态样板
- `app/src/stores/chat.ts:180` — SessionSummary type 加 `mode` 字段的样板
- `docs/ARCHITECTURE.md §2.2 ⑧a + ⑨` — ⑨ 关 5 道 check 设计
- `docs/IMPLEMENTATION.md §1` — 自研 agent core 决策(决定我们走自研 permission,不用 SDK)
- `docs/BACKLOG.md §4.2` — 5 个 Mode 分类
