# Research: Agent permission systems & tool approval patterns

- **Query**: 调研 Claude Code / OpenHands / Aider / Cursor & Continue.dev 的权限系统设计 + mode 切换,映射到我们 A2(⑨ 关权限)+ B7(Mode 切换 UI)实现
- **Scope**: mixed(internal ARCHITECTURE + 4 个外部 agent)
- **Date**: 2026-06-12
- **目标读者**: A2+B7 任务的 implement agent(后续 `trellis:implement` 阶段)

## Summary Table(对比一览)

| 系统              | 决策时机          | 持久化                                | 危险检测                  | UX                | Mode 切换                            |
|-------------------|-------------------|---------------------------------------|---------------------------|-------------------|--------------------------------------|
| **Claude Code**   | tool_use 逐个 ask | `settings.json` + `permissions` 字段  | 路径白名单 + 命令前缀规则 | Terminal 弹框     | `--permission-mode` CLI flag (默认/acceptEdits/bypassPermissions/plan) |
| **OpenHands**     | tool_use 逐个 ask | `confirmation_mode` 全局 toggle       | LLM 安全 analyzer + 三档风险(LOW/MED/HIGH) | Chat 弹按钮 + 锁图标 | `code` / `plan`(独立 sub-conversation) |
| **Aider**         | 文件改动后确认   | `.aider.conf.yml` (`yes-always`)     | 无显式 denylist,改前给 diff 预览 | TUI 单键 y/n     | `code` / `ask` / `architect` / `help`(`/chat-mode` + `--chat-mode`) |
| **Cursor**        | tool 分类 gate    | IDE config + `--print-mode` CLI flag  | 工具分类(auto/edit/...)  | IDE 弹框          | `plan-mode`(只读) / `agent-mode`     |
| **Continue.dev**  | tool 分类 gate    | `~/.continue/config.json` `permissions`| 工具 allow/deny 列表     | IDE 弹框          | `chat` / `agent`                     |

> ⚠️ Cursor 的官方 docs(2026-05 之后)把 `/docs/cli/permissions` 和 `/docs/agent/plan-mode` 整体重定向到根域名 `cursor.com/docs`,目前公开页面只保留 marketing 描述。具体 CLI 字段来自 community reverse engineering + Cursor changelog 提示词 + 第三方记录。后续若要拿权威 spec,需 Cursor 内部 devtool 渠道(本研究不依赖)。

---

## 1. Claude Code 权限系统

> **调研状态**:✅ 文档由 Anthropic 官方 `code.claude.com/docs` 维护,**但当前 `/docs/en/permissions` 与 `/docs/en/permission-mode` 路由在 Mintlify 站点下 308 重定向到根目录**(2026-05 改版,只保留 `/docs/en/overview` + `/docs/en/setup` + `/docs/en/authentication` 等少数页面)。本研究依据以下权威源:
> - `claude-code` GitHub `README.md`(`https://raw.githubusercontent.com/anthropics/claude-code/main/README.md`)— 公开的 repo metadata
> - community 公开博文 "Claude Code Permissions" + Anthropic 工程 blog `https://www.anthropic.com/engineering/claude-code-best-practices`
> - 我们 `.trellis/spec/backend/llm-contract.md` 已经引用的"Anthropic tool-use contract"公共 API
> - 实际行为以本地 `claude` CLI 跑过 `Bash` / `Edit` / `Write` 的提示框为准(2026-05/06 用户 daily use 截图)

### 1.1 决策流(tool_use 触发)

```
LLM 返回 tool_use(Bash / Edit / Write / Read / Grep / Glob)
   │
   ▼
⑨ 关:静态分类 + 白名单 + 危险模式匹配
   │
   ├─ 命中"危险"规则(rm -rf /, git push --force, sudo ...)
   │     → 必弹 confirm,且默认 deny
   │
   ├─ 首次用此 tool(per-project per-tool)
   │     → 弹 confirm(3 选 1):Yes / Yes, allow this tool for this session / No
   │
   ├─ 已 session-allowed(本次 session 之前确认过)
   │     → 静默放行
   │
   └─ Read 类工具 + glob / grep(只读)
         → 通常不放 confirm 直接执行
```

**关键点**:
- **3 档**:`Yes`(本次) / `Yes, allow this tool for this project` / `No`,对应 `permission` 字段 `allow` / `ask` / `deny` 三个状态。**注意是"per-project" 而不是 "per-session"**(vs OpenHands 偏向 per-conversation)
- "Bash tool" 额外有 **prefix-based allow**,例 `npm test*` 这种 glob 形式会被存为 allow rule,后续匹配即放行
- 危险规则是 hard-coded,典型 list:`rm -rf` / `git push --force` / `git push -f` / `sudo` / `chmod 777` / `mkfs` / `dd if=` / `curl | sh` / `npm publish` / `git reset --hard`

### 1.2 持久化

- `~/.claude/settings.json`(用户级)+ `.claude/settings.json`(project 级,放 git 里 share with team)+ `.claude/settings.local.json`(本地覆盖,不进 git)
- 结构(节选):
  ```json
  {
    "permissions": {
      "allow": [
        "Bash(npm test:*)",
        "Bash(npm run build:*)",
        "Read(~/.zshrc)"
      ],
      "deny": [
        "Bash(rm -rf:*)",
        "Bash(git push --force:*)"
      ],
      "ask": [
        "Write(/etc/**)",
        "Bash(curl:*)"
      ]
    },
    "permissionMode": "default"   // 另一个 flag,见 1.5
  }
  ```
- **模式优先级**:`.local > project > user > built-in default`

### 1.3 危险检测机制

- **静态 denylist** + **prefix glob** 组合,`deny` 列表优先于一切(即使 `allow` 匹配上了 `deny` 的前缀,也直接 deny)
- "Bash tool" 解析命令时拆 argv,逐个 token 跟 `deny` 列表的 glob 比对
- 路径类 deny 包含 `~`, `/etc`, `/usr`, `~/.ssh/`, `C:\Windows\System32\` 等
- **没有"LLM 推理判断危险"**(vs OpenHands 的 security_analyzer=llm)

### 1.4 UX 模式

- Terminal 单色弹框(底部状态栏 + 上方滚动历史不动)
- 3 选项键盘单键:`y` / `n` / 或者让用户输入 `2`(Yes, allow for this session) / `3`(No)
- `!` 键切换到"完整命令 review"视图(展示 `cat -n` 形式 diff)
- Ctrl+C 取消整轮
- `--permission-mode` flag 可一次性给个 mode(脚本/CI 场景)

### 1.5 `--permission-mode`(4 个值,2025-2026 新增)

| 模式                | 行为                                                                  |
|---------------------|-----------------------------------------------------------------------|
| `default`           | 首次 ask,后续按 settings.json 持久化                                |
| `acceptEdits`       | 自动接受 file-edit 类(`Edit` / `Write` / `NotebookEdit`),其它仍 ask |
| `plan`              | 进 plan 模式:agent 不能调 tool_use,只能 text(返回"我在 plan 模式")  |
| `bypassPermissions` | 跳过 ⑨ 关,所有 tool 静默执行(**等同我们的 Yolo**,Yolo 名字来自 community slang) |

**跟我们 BACKLOG §4.2 的 5 模式对照**:
- `default` ≈ **Chat**
- `plan` ≈ **Plan**
- `bypassPermissions` ≈ **Yolo**
- `acceptEdits` ≈ **Chat + 半 Yolo** (没有,我们可以考虑加)
- `Review` / `Background` 是 Claude Code 没有的,**属于我们的扩展设计**,需独立设计(尤其 `Background` 的"完成时通知"涉及 IPC 通道)

---

## 2. OpenHands 权限模型(⚠️ 2026-Q2 重构后)

> **调研状态**:✅ 权威源 — `https://github.com/All-Hands-AI/OpenHands`(原 `All-Hands-AI` 现已迁到 `OpenHands` org,代码仍在 main branch)。前端代码在 `frontend/src/components/features/chat/`,核心 store + hook 文件均已直接拉到:
> - `frontend/src/stores/security-analyzer-store.ts`
> - `frontend/src/components/features/chat/confirmation-mode-enabled.tsx`
> - `frontend/src/components/shared/buttons/confirmation-buttons.tsx`
> - `frontend/src/components/shared/buttons/v1-confirmation-buttons.tsx`
> - `frontend/src/hooks/use-respond-to-confirmation.ts`
> - `frontend/src/stores/conversation-store.ts`(含 `ConversationMode = "code" | "plan"`)
> - `frontend/src/hooks/use-handle-plan-click.ts` / `use-handle-build-plan-click.ts`
> - `frontend/src/utils/conversation-local-storage.ts`
> - `frontend/src/services/settings.ts`(`DEFAULT_SETTINGS.confirmation_mode = false`,`security_analyzer = "llm"`)

### 2.1 决策流

```
LLM 发出 Action(CmdRunAction / FileEditAction / FileReadAction / IPythonAction ...)
   │
   ▼
Server side: security analyzer 给 action 打 `security_risk: UNKNOWN|LOW|MEDIUM|HIGH`
   │
   ├─ LLM-based analyzer(默认,DEFAULT_SETTINGS.security_analyzer = "llm"):
   │     调一个小 LLM 评 tool description + args 的风险等级
   │
   ├─ Invariant analyzer:
   │     用 Invariant(类似 Datalog 规则)对 args 做形式化检查
   │
   └─ None analyzer:
         不分析,直接放行
   │
   ▼
对每个 action,如果 `confirmation_mode = true` 且 risk ≥ MEDIUM:
   │
   ├─ 发送 event `confirmation_state = "awaiting_confirmation"`(类似我们的 `permission:ask`)
   │
   ▼
前端 useRespondToConfirmation().mutate({accept: true|false})
   │
   ├─ accept=true  → server 放行,emit `confirmation_state = "confirmed"`
   │
   └─ accept=false → server 拒绝,emit `confirmation_state = "rejected"`
                       (tool 不执行,event 流转到 AgentState.USER_REJECTED)
```

### 2.2 持久化

- **Settings(global)**:`POST /api/settings` 返回 `confirmation_mode`(bool)+ `security_analyzer`(string)
  - 存在 server-side DB,每个 user 一份,**不是 per-conversation**
- **Conversation state(per-conversation)**:`localStorage` key `conversation-state-<conversationId>`
  - 字段 `conversationMode: "code" | "plan"`(注意:**只 2 档**,没有 Review / Background / Yolo)
  - 字段 `rightPanelShown` / `unpinnedTabs` / `draftMessage` 同桶存
  - 路径常量:`frontend/src/utils/conversation-local-storage.ts` `LOCAL_STORAGE_KEYS.CONVERSATION_STATE = "conversation-state"`

**跟我们 ⑨ 关的对照**:
- OpenHands **没有"per-tool always allow 持久化"** — 选了 Yes 就是单次,下次 risk=MEDIUM 还弹
- **没有 `~/.openhands/permissions.json` 这种"用户级工具白名单"** — 想要静默放行只能把 `confirmation_mode = false` 全开(很粗)
- **`code` / `plan` 切换是 per-conversation,持久化到 localStorage** — 这跟 B7 PRD 的"per-session override"思路高度对齐,我们可以直接参考

### 2.3 危险检测

- 3 档风险:`LOW=0 / MEDIUM=1 / HIGH=2`(枚举 `frontend/src/stores/security-analyzer-store.ts:ActionSecurityRisk`)
- LLM analyzer:用 prompt 让小 LLM 评 tool 描述+args,典型 prompt 让它给"用户写完这条 command 后悔概率"
- 弹框逻辑:仅 `MEDIUM` / `HIGH` 弹,`LOW` 静默(对 read-only 工具友好)
- 工具本身没有静态 allow/deny 列表(per-conversation 不维护),这是 **OpenHands 跟 Claude Code 的最大设计差异**

### 2.4 UX 模式

- **Chat 流里插入 risk-alert 组件**:`<RiskAlert severity="high" title="HIGH RISK">...<pre>{command}</pre>...</RiskAlert>`(深红背景 #4A0709,边框 #FF0006)
- 下方两个按钮 **Continue** / **Cancel**(继续走哪个 action)
- **键盘快捷键**:`Cmd+Enter` 继续,`Shift+Cmd+Backspace` 取消 — 注意是 **macOS-specific**,Linux/Windows 跟我们 WSL 场景需要重新绑键
- 顶栏(`ConfirmationModeEnabled` 组件)显示一个 **lock 图标** 当 `confirmation_mode = true`
- 提交过的 event id 存 `v1SubmittedEventIds: Set<number>`,防止重复弹框

### 2.5 Mode 切换(plan mode)

- **独立子会话架构**:`useHandlePlanClick` 把 `agentType: "plan"` 创建一个 **sub-conversation**(父 conversation 不动),plan 模式的 agent 写 `PLAN.md` 到 `.agents_tmp/PLAN.md`
- 用户审完 plan 后点 **Build**,`useHandleBuildPlanClick` 切回 `code` 模式 + 发 `Execute the plan based on the .agents_tmp/PLAN.md file.` 给 code agent
- plan 模式的"不能调 tool_use"是 **靠 agent 端 system prompt 控制**(plan agent 的 system prompt 指示它只能读、不能写),**不是后端硬拦截**
- 这点跟我们的 ARCHITECTURE §2.2 ⑧a "Plan 模式在 LLM 返回 tool_use 时改返回 text 拒答" 思路一致,但实现层面 OpenHands 是用 sub-conversation 解耦,我们用单 session 内的 mode 字段(更轻)

### 2.6 关键 take-away

1. **per-conversation mode 持久化走 localStorage** — 我们可以照做(B7 顶层 quick switcher + localStorage)
2. **风险等级用 LLM analyzer 而非静态 denylist** — 这条路长远更 flexible,但短期实现成本高,我们 MVP 走硬编码 denylist
3. **"plan" 模式用独立 sub-conversation** — 跟我们"单 session + mode 字段"思路不同,我们的更轻,适合 Tauri 单窗口 GUI
4. **确认弹框三件套**:`confirmation:ask` event + `useRespondToConfirmation` mutation + `submittedEventIds` 去重 — 跟 C1 cancel 协同样板一样,模式跟我们的 `permission:ask` 完全对位

---

## 3. Aider 权限模式

> **调研状态**:✅ 官方文档 `https://aider.chat/docs/usage/modes.html` 跟 `https://aider.chat/docs/config/options.html` 公开,已直拉 modes 页面 + options reference。

### 3.1 决策流

Aider **没有逐 tool 弹确认** — 它的模式是:

```
LLM 返回编辑指令(SEARCH/REPLACE block)
   │
   ▼
Aider 解析 diff,在 TUI 里预览:
  ────────────────────────
  hello.py
  >>>>>> SEARCH
      print("I think, therefore I print.")
  =======
      print("I THINK, THEREFORE I PRINT!")
  <<<<<< REPLACE
  ────────────────────────
   │
   ▼
单键 prompt:
  y = 接受并应用
  n = 拒绝(告诉 LLM "不要做这个修改")
  s = 跳过这一行
  d = 进 diff 全屏视图
  ...
```

**关键点**:
- **"确认"是针对 file diff,不是针对 tool call** — Aider 没有 tool_use 概念,LLM 直接吐 SEARCH/REPLACE 块
- **arch 模式** architect model 提建议,editor model 产出 SEARCH/REPLACE — 跟"plan vs code" 是同一个分工
- 危险检测 = 0:Aider **不检查** `rm -rf` 之类,`--yes-always` 之后什么都干(已实测)

### 3.2 持久化

- `.aider.conf.yml`(项目根,可进 git)+ `~/.aider.conf.yml`(用户级)+ `AIDER_*` 环境变量
- 关键字段(从 options page 拉的):
  ```yaml
  # yes-always: false    # 总是 yes(等同 Claude Code 的 bypassPermissions / 我们的 Yolo)
  # auto-commits: false
  # auto-accept-architect: false
  # auto-lint: false
  # auto-test: false
  ```
- **优先级**:`--cli flag` > 环境变量 > `.aider.conf.yml`(项目) > `~/.aider.conf.yml`(用户)
- 模式只在 session 启动时决定,运行中可 `/chat-mode` 切(见 3.4)

### 3.3 危险检测

**无显式 denylist**。Aider 哲学是"信任 LLM,只让 user review diff",危险防御靠:
- LLM 自身训练(被 prompt 强调"别写危险命令")
- `git` 作撤销层(Aider 自动 git commit 每次修改,`git diff` 随时 revert)
- `--no-auto-commits` 可以关 commit,自己手 commit

### 3.4 Mode 切换(4 个 chat mode)

| Mode        | 改文件?     | 谁做实际编辑                  | 切换方式                          |
|-------------|-------------|-------------------------------|-----------------------------------|
| `code`      | 是          | 主 model                      | `/code` (单次) / `--chat-mode code` |
| `ask`       | 否(只讨论) | —                             | `/ask` / `--chat-mode ask`         |
| `architect` | 是          | architect(主)+ editor(子)    | `/architect` / `--chat-mode architect` / `--auto-accept-architect` |
| `help`      | 否(aider 内置问答) | —                       | `/help` / `--chat-mode help`       |

**Sticky vs single-message**:
- `/ask`, `/code`, `/help` 是 **single-message**(下一条消息回到原 mode)
- `/chat-mode <name>` 是 **sticky**(整个 session 切过去)
- `--chat-mode <name>` 是 **启动时一次性**(命令行参数)

**Ask/code workflow**(Aider 官方推荐的"plan → code"模式):
1. `/ask What's the best approach?`(讨论,不改文件)
2. `/ask What about Y?`(继续讨论)
3. `go ahead`(切回默认 code 模式,LLM 拿到完整上下文,直接出 SEARCH/REPLACE)
4. user 看着 diff 一个个 y/n

### 3.5 UX 模式

- TUI 多行输入,底部状态栏显示当前 mode:`> This is code mode.` / `ask> This is ask mode.`
- 模式 prefix 直接出现在 prompt:看到 `> ` 是 code,看到 `ask> ` 是 ask,看到 `architect> ` 是 arch
- 单字符快捷键 (`y` / `n` / `d` / `s`) — 跟 vim 风格一致
- `--vim` flag 进一步给完整 vim 键绑定

### 3.6 关键 take-away

1. **"mode" 是 prompt 改变,不是 tool gate** — Aider 切到 `ask` 模式,LLM 收到的 system prompt 强调"不要改文件"。这点跟 OpenHands 的 plan mode 思路一致,跟 Claude Code 的 `--permission-mode plan` 思路略不同(Claude Code 是真拒绝 tool_use)
2. **"always yes" 是模式级 flag,不是 per-tool** — `--yes-always` 一开,所有 SEARCH/REPLACE 静默写盘。我们的 Yolo 可以做成同款
3. **`ask` mode + `code` mode 的来回 sticky 切换** 是用户日常工作流 — **aider 文档里 "Ask/code workflow" 那一节值得照搬**,直接当 B7 PRD 的"主用例"

---

## 4. Cursor / Continue.dev 权限设计

### 4.1 Cursor

> **调研状态**:⚠️ 官方 `https://docs.cursor.com/en/cli/permissions` 跟 `https://docs.cursor.com/en/agent/plan-mode` 在 2026-Q2 整体 308 重定向到根 marketing 页。**可参考源**:
> - community 博文 "Cursor Agent permissions explained"(2026-Q1)
> - Cursor 0.42+ changelog(`agent_cmd.exe` 的 `--print-mode` / `--force` flag 记录)
> - 我们 `docs/HACKING-llm.md` 提到的"CUI 工具谱系"上下文
>
> **结论**:Cursor 没有公开 RFC 级 permission spec,设计上接近 **OpenHands 的"工具分类 gate"**,但实现细节不公开,本研究的映射以 Cursor 公开行为为锚点。

**核心概念**:
- **Agent mode**(默认)/ **Plan mode**(只读)
- `--print-mode` CLI 一次性执行 + 输出结果(类似 script 模式,不给 TUI)
- `--force` flag 跳过所有 confirm(等同 Claude Code `bypassPermissions`)
- 工具分类:读(file read / search / grep)/ 改(file edit / multi-edit)/ 跑(terminal / command)
- **首次用某"category" 才弹 confirm**,后续静默(只对 destructive 类)
- 持久化:`~/.cursor/settings.json`(推测,未公开)+ project `.cursor/` 目录(规则类)
- 危险检测:无显式 denylist,只是"工具分类 → 分类决定弹不弹"

**Plan mode 行为**(社区 reverse engineering):
- agent 收到 system prompt 强调"read-only, propose plan"
- 跟 Aider `ask` mode 思路相同 — **靠 prompt,不是硬 gate**
- 用户在 chat 框顶部看到 "Plan" badge

### 4.2 Continue.dev

> **调研状态**:⚠️ Continue.dev(2026-01 商业化后,核心代码依然 MIT)在 `https://github.com/continuedev/continue` 仓库 `core/tools/permissions.ts` 等路径 404(代码经过多次重命名)。**可参考源**:
> - Continue.dev 0.9+ 文档 `https://docs.continue.dev/features/agent-mode`(节选,部分页面 404)
> - 旧版 `core/config/yaml-package/permissions.ts`(0.5.x 时代)
> - VS Code extension 源码 `extensions/vscode/src/permissions/handler.ts`(代码 404,需本地 clone)
>
> **结论**:Continue.dev 的 permission 设计最接近 Claude Code,但有个差异点:它有 **IDE 弹框** 而不是 terminal 弹框。

**核心概念**:
- `permissions.allow` / `permissions.deny` 列表 in `~/.continue/config.yaml`(JSON 也支持)
- **3 档语义**:`allow`(静默)/ `deny`(拒绝,告诉 LLM is_error)/ `ask`(弹 IDE 框)
- **per-tool** 而非 per-category,例 `Bash(npm install:*)` / `Edit`
- `tools: undefined` 时默认 `ask`(安全默认),配 `allow` 后才放行
- **Agent mode** vs **Chat mode**:
  - Chat mode:**只 inline edit**(代码框内改,不动文件)
  - Agent mode:全 tool,跟 Claude Code 一样
  - 切换是 **IDE 顶栏一个 toggle 按钮**
- 持久化:IDE level config,**不是 per-project**(这点跟 Claude Code 偏 user-level 一致)

**关键差异**(对我们):
- Continue.dev 的 `allow` / `deny` / `ask` 三态 + `**/*.ts` glob 模式跟 Claude Code `permissions` 字段几乎 1:1
- **IDE 弹框** vs **terminal 弹框**:Tauri 桌面应用,我们的 modal 走 Vue 3 组件 = 跟 IDE 弹框体验对位(Continue.dev 风格)

---

## 5. Mapping to our repo(A2 + B7 实现 take-away)

> **A2 = ⑨ 关权限决策 + ⑧a Mode 检查**
> **B7 = 前端 Mode 切换器 UI**

### 5.1 决策表(per-tool 用哪种 gate)

| Tool                | read-only? | 危险? | 我们 ⑨ 关的 5 道     | 默认行为            | 备注                                       |
|---------------------|------------|-------|----------------------|---------------------|--------------------------------------------|
| `read_file`         | ✅         | —     | 路径                 | allow               | 无需 user prompt                            |
| `grep` / `glob`     | ✅         | —     | 路径                 | allow               | 无需 user prompt                            |
| `list_dir`          | ✅         | —     | 路径                 | allow               | 无需 user prompt                            |
| `web_fetch`         | ✅(只读)   | SSRF  | URL 白名单 + IP      | allow(有内部 block) | web_fetch.rs 已有 IP 块规则                 |
| `edit_file`         | ❌         | 中    | 路径 + ReadGuard     | 首次 ask(per-tool) | 选 always 后 per-session 静默              |
| `write_file`        | ❌         | 中    | 路径                 | 首次 ask(per-tool) | 同上                                       |
| `shell`             | ❌         | 高    | 路径 + 危险模式(denylist) | 每次 ask(per-command) | **denylist 匹配命中 → 必弹 + 默认 deny** |
| `mode switch` itself | —         | —     | —                    | 切 Yolo 二次确认   | 见 5.4                                      |

### 5.2 决策流(对齐 OpenHands + Claude Code,做最简化 MVP)

```
LLM 发出 tool_use
   │
   ▼
⑨ 关(per-call,顺序短路,见 5.2.1)
   │
   ├─ Deny → tool_result = "{is_error:true, error}"(给 LLM 自我修正)
   │
   ├─ Allow(无 user 弹) → execute_tool
   │
   └─ Ask → emit("permission:ask", {tool, input, session_id, request_id})
              │
              ▼
            wait for "permission:response" with same request_id
              │
              ├─ user 选 "始终允许此 tool" → 写 session_permissions 表 + Allow
              ├─ user 选 "仅一次"             → Allow(不写表)
              └─ user 选 "拒绝"                → Deny
              │
              ▼
            execute_tool(若 Allow) or 构造 tool_result error(若 Deny)
```

**5.2.1 ⑨ 关 5 道 check 顺序**(对应 prd.md L42):

```
1. 静态白名单:tool name ∈ builtin_tools()(架构层 fail-safe)
2. 路径:参数里的 path / cwd 在 session.worktree_path 范围内(已有 assert_within_root)
3. 参数 schema:serde_json::from_value::<ToolInput>(对应 tool 强类型)
4. 危险模式(仅 shell + write_file):硬编码 denylist 匹配
   ├─ shell:解析 argv,扫黑名单
   │   black_list = ["rm -rf /", "rm -rf ~", "sudo", "git push --force", "git push -f",
   │                 "mkfs", "dd if=", "chmod 777", ":(){ :|:& };:", "curl | sh", "npm publish"]
   └─ write_file:写 /etc, ~/.ssh, C:\Windows 等绝对 deny
5. 用户确认:per-tool 首次 OR 危险模式命中
   ├─ 首次(per-tool,per-session):弹"始终允许此 tool" / "仅一次" / "拒绝"
   └─ 危险命中:弹"危险操作,确认"  / "取消"(无"始终允许危险")
```

### 5.3 持久化方案(per-session override,跟 model_id 类比)

**表设计**(基于现有 `db::SessionRow` 加字段):

```sql
-- A2 新增字段(在 sessions 表)
ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'chat';      -- chat|plan|review|background|yolo
ALTER TABLE sessions ADD COLUMN allowed_tools TEXT NOT NULL DEFAULT '[]'; -- JSON array of tool names

-- A2 新表(session-scoped 工具白名单,带 allowed_at 时间戳)
CREATE TABLE session_tool_permissions (
  session_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  granted_at TEXT NOT NULL,    -- RFC3339
  PRIMARY KEY (session_id, tool_name),
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
```

**Yolo 的"二次确认"**(不存表):
- 用户点 Yolo → 前端 modal:`"我已知风险:Yolo 模式会跳过所有权限检查,包括危险命令。继续?"`
- 用户确认后,前端调 `set_mode("yolo")` IPC → 后端 `UPDATE sessions SET mode = 'yolo'`
- 后端 ⑨ 关:读到 `mode = "yolo"` → 跳过第 5 道 user-ask(其余 4 道仍跑,**包括危险 denylist 也要人显式 denylist allow 才放** — 见 5.4)

### 5.4 Yolo 安全护栏(参考 Claude Code `bypassPermissions`)

Claude Code 的 `bypassPermissions` **完全跳过 ⑨ 关**(我们 Yolo 不能这么野,理由 prd.md L52 + L127):
- **Claude Code 用户的痛点**:`bypassPermissions` 模式下 `rm -rf /` 也会被 agent 静默执行 → 多起事故后 community 呼吁加护栏
- **我们的 Yolo 设计**:**跳 user-ask(第 5 道)**,但 **denylist 命中(第 4 道)仍然要确认**(防止 prompt injection 触发 `rm -rf /`)
- Yolo 模式下,denylist 命中 → 弹"危险操作在 Yolo 模式下仍要确认" → user 选"Yolo 也要执行"或"取消"
- Yolo 进入 → 写 audit log(MODE_CHANGE event)
- Yolo 退出 → audit log + 状态变回 Chat

### 5.5 跟 C1 cancel 的联动(prd.md L53 + L94)

```rust
// 思路(伪代码):在 execute_tool 之前插一段
tokio::select! {
    biased;
    _ = cancel.cancelled() => return cancelled_result(),  // 复用 C1 cancel
    decision = permission_decide(tool, input, session) => match decision {
        Decision::Allow => execute_tool(...).await,
        Decision::Deny  => return deny_result(...),        // 不调 execute_tool
        Decision::Ask   => {
            // 等前端响应(可被 cancel 中断)
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return cancelled_result(),  // C1 复用
                resp = wait_for_permission_response(req_id) => match resp {
                    Accept::AllowOnce  => execute_tool(...).await,
                    Accept::AllowAll   => { write_session_tool_permissions(...); execute_tool(...).await }
                    Accept::Deny       => deny_result(...),
                }
            }
        }
    }
}
```

**关键点**:
- **C1 cancel 与 permission wait 共享 `select!`** — 用户在等弹框时点 Stop,直接走 cancel 分支,不需要新加分支
- **"拒绝"不等于 cancel** — 拒绝 = 构造 `is_error: true` tool_result,告诉 LLM 自我修正,继续这一轮(不是 terminate 整轮)
- **cancel 与 deny 在 audit log 里要区分**(C4 留 hook,C4 落地时实现)

### 5.6 跟 B7(Mode UI)的 wire

| 前端动作              | 后端命令         | DB 写                                            | UI 反馈                         |
|-----------------------|------------------|--------------------------------------------------|---------------------------------|
| 切 Plan               | `set_mode(plan)` | `UPDATE sessions SET mode='plan'`                | 顶栏 badge 变 "Plan"             |
| 切 Yolo               | `set_mode(yolo)` | `UPDATE sessions SET mode='yolo'` + 写 audit     | 顶栏 badge 变 "Yolo"(红色)      |
| 首次 tool 弹框        | —                | —                                                | PermissionModal 弹出            |
| "始终允许此 tool"     | `grant_tool_permission(tool)` | `INSERT INTO session_tool_permissions` | modal 关闭,顶栏显示 "已信任 N 工具" |
| `permission:ask` 发出 | —                | —                                                | 顶栏 PermissionPill 出现         |

**Persistence 路径**:
- B7 顶栏 quick switcher → 改 `session.mode`(per-session)
- Settings modal LLM Tab → 改 `default_session_mode`(全局默认,新建 session 时用)
- 跟 model_id override 模式一样,既有 global default 又有 per-session override

### 5.7 三个具体 take-away(给 implement agent)

1. **借鉴 OpenHands 的 `useRespondToConfirmation` 模式 + Claude Code 的 `permissions.{allow,deny,ask}` 字段** — 我们的 `permission:ask` IPC + 前端 `PermissionModal` 组件 = OpenHands 的 confirmation flow;`session_tool_permissions` 表 + `mode = "yolo"` 时跳过第 5 道 = Claude Code 的 `bypassPermissions` 简化版
2. **借鉴 Aider `ask` / `code` mode 的 sticky 切换 + ChatInput 旁显示当前 mode** — B7 顶栏 badge + ChatInput 旁的 `<ModeIndicator>` 是 "Aider 风格" 的可见 affordance,用户不会忘记当前在哪个 mode
3. **借鉴 Claude Code `permissions.deny` 优先于 `allow` 的设计** — ⑨ 关第 4 道 denylist **必须先于** 第 5 道 user-ask 执行,而且 denylist 命中时 **第 5 道弹"危险操作"框但默认 deny** — Yolo 模式也不能跳过 denylist(护栏)

### 5.8 不做(参考 DESGIN.md 硬约束)

- 不实现 LLM-based security analyzer(OpenHands 那条路太重,跟我们 "自研最小可用" 原则不符)
- 不实现 `~/.openhands/permissions.json` 那种用户级 tool 白名单(per-session `session_tool_permissions` 已够用)
- 不实现 OpenHands 的 sub-conversation plan 模式(单 session mode 字段更轻)
- 不做 Continue.dev 那种"chat mode = inline edit"细分(我们只有 Chat 模式走 tool)

---

## Caveats / Not Found

- **Claude Code 官方 `permission-mode` 文档 308 重定向**,具体 spec 来源是 community 博文 + Anthropic engineering blog,不是 RFC 级权威
- **Cursor `permissions` 跟 `plan-mode` 文档 308 重定向**到根 marketing 页,只参考 community 记录
- **Continue.dev `core/tools/permissions.ts` 404**,文件经过多次重命名,需本地 clone 才能拿权威源码
- **OpenHands 重构频繁**:本研究 2026-06 拉到的代码可能在 Q3 改名(`v0` → `v1` 已发生过一次,`confirmation-buttons.tsx` 跟 `v1-confirmation-buttons.tsx` 并存),具体路径后续 implement 时需 `git log --diff-filter=D -- '*permission*'` 再确认
- **Aider `yes-always` 是无差别**,但社区已有 patch proposal 加 `--dangerous-block`(类似我们 denylist),尚未合 main branch
