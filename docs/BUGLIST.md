# BUGLIST — WebUI 全量测试缺陷跟踪

> **来源**:2026-08-29 对 `http://localhost:7456/chat` 做的浏览器黑盒全量功能测试(用户视角,15 章节覆盖)。
> 原始报告(33 项记录 + 40 余张截图)在测试机 `C:\Users\kaijin\ZCodeProject\everlasting-webui-test\2026-08-29-webui-fulltest-report.md`(仓库外)。
> **甄别方式**:每项对照源码逐条考证(定位到行级根因)+ 关键截图核验 + SQLite 只读抽查;区分「真缺陷 / 设计特性 / 测试方法局限 / 功能建议」。
> **用法**:§2 是待修复清单,修复一项就把状态改 ✅ 并填提交引用;§3 已判定设计如此,除非产品主张变更不再重开;§4 待人工复核;§5 功能建议池。编号沿用原报告 CHx-y,可交叉查证。

---

## 1. 结论速览

原报告 33 项记录的甄别分布:

| 判定 | 数量 | 条目 |
|---|---|---|
| A. 真缺陷,进 §2 跟踪修复 | 10 | CH4-1、CH3-1、CH4-2、CH2-1、CH5-2、CH13-1、CH12-1a、CH12-1b、CH3-3/7-3、CH9-1 |
| B. 设计如此,关闭(§3) | 14 | CH1-1、CH1-2、CH2-3、CH2-4、CH4-3、CH4-4、CH5-3、CH6-1、CH7-1、CH7-2、CH8-1、CH10-1、CH11-2、CH14-2 + CH13 观察项 |
| C. 测试方法局限,人工复核后全部关闭(§4) | 2 | CH5-1(Shift+Enter 误报)、CH14-1(焦点环达标) |
| D. 功能建议,非缺陷(§5) | 8 | CH4-5、CH5-4、CH11-1、CH12-2、CH3-2、CH7-4、CH8-2、CH4-3 视觉部分 |

测试确认**可用**的主干:核心对话链路、工具调用与权限(含 Yolo kill list)、定时任务(真实触发)、群聊、子代理、设置体系、Trace/审计/配额面板、移动端响应式。

---

## 2. 待修复清单(A 类)

> 状态:⬜ 待修复 · 🔧 修复中 · ✅ 已修复(填提交) 。按严重级别排序。

| 编号 | 级别 | 问题 | 根因位置 | 状态 | 修复提交 |
|---|---|---|---|---|---|
| CH4-1 | **P1** | 编辑已发消息报「消息缺少 seq 无法编辑」 | `useMessageEditing.ts:164,233` | ✅ 已修复 | 61c0dc8 |
| CH3-1 | **P1** | 关项目 Tab 后隐藏列表不更新,重新添加被拒 | `stores/projects.ts:312-327` | ✅ 已修复 | 0536914 |
| CH4-2 | P2 | 消息 Markdown 有序/无序列表无标记 | Tailwind preflight + `MessageItem.vue:1624` | ✅ 已修复 | f030e4d |
| CH2-1 | P2 | 含消息的群聊会话删除永不弹确认 | `SessionList.vue:302-305` + 群聊 preview 恒空 | ✅ 已修复 | 8388dc7 |
| CH5-2 | P3 | /clear 后「累计耗时」与空态提示同屏自相矛盾 | `stores/chat.ts:450-463` | ✅ 已修复 | a551314 |
| CH13-1 | P3 | 审计/Trace 事件时间显示 UTC,与界面本地时间不一致 | `utils/audit.ts:545-552` | ✅ 已修复 | 1ada545 |
| CH12-1a | P3 | 搜索消息命中分组的会话头视觉像可点,实际无 handler | `SearchModal.vue:370-374` | ✅ 已修复 | 44cf49b |
| CH12-1b | P3 | 搜索预览「在主窗口打开」不定位到命中消息(不消费 seq) | `SearchModal.vue:245-248` | ✅ 已修复 | 44cf49b |
| CH3-3/7-3 | P3 | 错误 toast 暴露内部传输层名 `[httpTransport]` | `transport/http.ts:226-238` | ✅ 已修复 | 15dfdcf |
| CH9-1 | P3 | 记忆弹窗暴露内部文档路径 `docs/IMPLEMENTATION.md` | `MemoryPreview.vue:479-485` | ✅ 已修复 | a707218 |

### CH4-1 编辑消息「缺少 seq」(P1)

- **现象**:对会话第一条用户消息执行 编辑→保存,报「⚠ 消息缺少 seq 无法编辑」;刷新后复现。
- **根因**:守卫用 falsy 判断 `if (!message().seq)`(`useMessageEditing.ts:164` 编辑保存、`:233` 编辑器内重发)。而会话第一条消息的 seq 恰为 `0`——后端空会话起点即 0(`app/src-tauri/src/agent/chat_loop/init.rs:178-184`),前端预盖章同样算出 0(`chatSendActions.ts:328-331`)。菜单入口的门槛却是 `seq !== undefined`(`MessageItem.vue:629`),0 能进菜单 → **每个会话的第一条消息必现**,与 transport 无关。
- **修复方向**:守卫改 `message().seq == null`(或 `typeof !== "number"`),两处同改。
- **回归验证**:新建会话发一条消息,编辑该消息并保存;再取一条 seq≥1 的消息回归编辑/重发正常路径。

### CH3-1 关项目 Tab 后入口丢失(P1)

- **现象**:关 Tab 后「已隐藏项目」列表不含该项目;通过「添加项目」输入原路径报 `already exists`;**刷新后**才出现在隐藏列表方可恢复——窗口期内无任何 UI 途径找回。
- **根因**(两洞叠加):
  1. `hideProject`(`stores/projects.ts:312-327`)只调 `loadProjects()`,不调 `loadHiddenProjects()`;对比 `unhideProject`(`:338-351`)两者都调,明显不对称。
  2. 「添加项目」的隐藏恢复逻辑存在(`registerPickedPath` `:276-289`,RULE-FrontProj-001),但 lazy 补拉条件是 `hiddenProjects.length === 0`(`:261-263`)——启动时已有其他隐藏项目时列表是陈旧的,匹配不到就直走 `create_project` → 后端 UNIQUE 冲突(`db/projects.rs:56-58`)。
- **修复方向**:`hideProject` 内补 `await loadHiddenProjects()`;或把 lazy 补拉条件改为无条件重拉。
- **回归验证**:有多项目 + 已有隐藏项目的情况下,关一个 Tab → 隐藏列表立即可见 → 「重新打开」恢复;关闭后立即「添加项目」同路径应走恢复分支而非报错。补 `projects.test.ts` 用例(现仅覆盖 add 的 lazy 兜底,`projects.test.ts:225-246`)。

### CH4-2 列表标记丢失(P2)

- **现象**:助手消息里有序列表无序号、无序列表无圆点,仅剩缩进(截图证实)。
- **根因**:`style.css:17` `@import "tailwindcss"`——Tailwind v4 preflight 全局重置 `ul, ol { list-style: none; margin: 0; padding: 0 }`;`MessageItem.vue:1624-1641` 的 `.msg__markdown ul/ol` 只补回了 margin/padding,没补 `list-style`。渲染管线本身正常(marked→DOMPurify 保留列表标签)。
- **修复方向**:`.msg__markdown :deep(ul) { list-style: disc }`、`ol { list-style: decimal }`。
- **回归验证**:含有序+无序+嵌套列表的消息渲染;顺带检查 `SearchPreviewBody`(它自己显式做了 `list-style: none`,是自绘列表,不受影响)。

### CH2-1 群聊删除永不弹确认(P2)

- **现象**:删除含 38 条消息的群聊会话,无确认弹窗直接删;普通会话有消息时弹确认。
- **根因**:确认条件是 `preview` 非空(`SessionList.vue:302-305`),preview 取最后一条 `role='user'` 消息的 text(`session_crud.rs:148-153` COALESCE 子查询)。DB 实证:群聊会话的 user 消息 `text` 恒为空串(18 条 user 消息 length 全 0,群聊用户输入不走该字段)→ preview 恒空 → **群聊无论多少消息都走免确认分支**。空会话免确认本身是有意设计,群聊是被连带的。
- **修复方向**:preview 判空对群聊失效——改用消息数判断(如 `msg_count > 0` 需后端 list_sessions 带出),或群聊场景单独给确认条件。
- **回归验证**:含消息群聊删除弹确认;空群聊/空普通会话仍免确认。

### CH5-2 /clear 后统计自相矛盾(P3)

- **现象**:/clear 后底部仍显示「LLM 4.5s / 7.1K」,延迟弹窗同时出现「累计 4.5s」与「本次 session 还没有 LLM 耗时数据」;刷新后自愈。
- **根因**:累计值存 `sessionTotalLatencyMs` Map(`stores/chat.ts:450-463`),/clear 路径(`chatSessionActions.ts:258-281`)只删 DB 行 + evict controller,不清该 Map;轮次列表从消息派生为空 → 两个数据源口径脱节。「累计按 session 生命周期」口径本身没错,错在清空后 UI 仍展示。
- **修复方向**:/clear 时同步清该 session 的 latency/token 累计 Map(与 DB 语义对齐——DB 行已删,刷新后本来就归零)。
- **回归验证**:/clear 后底部统计与弹窗一致;切换会话来回后累计值不串。

### CH13-1 审计/Trace 时间显示 UTC(P3)

- **现象**:审计条目时间 06:54,界面其余为本地 15:xx。
- **根因**:`utils/audit.ts:545-552` 直接切片 SQLite `datetime('now')` 的 UTC 串,注释自己承认;同仓库 `utils/time.ts:26-34` 有正确的本地化转换且写了「slicing 会漂移 ~8h」的警示,未复用。`TraceEventItem` 是 `AuditLogItem` 薄包装,连带同病(TurnCard 本身不显示墙钟时间,不受影响)。
- **修复方向**:`formatTimeOfDay` 改走 `utils/time.ts` 的 Date 本地化。
- **回归验证**:审计日志/Trace 事件时间与本地时钟一致。

### CH12-1a/1b 搜索交互两处(P3)

- **1a**:消息命中分组里的**会话标题行**是纯 div 无 handler(`SearchModal.vue:370-374`),视觉上却像可点——本次测试正是点了它判定「无响应」。设计交互是:标题命中行点击跳会话(已实现,`:351-357`);消息命中行点击开弹窗内预览定位(已实现,`:377` + `SearchPreviewBody.vue:91-95` scrollIntoView+高亮)。
- **1b**:预览的「在主窗口打开」(`:415-421`)复用 `openInMainWindow`,但该函数只切会话**不消费 `preview.seq`**(`:245-248`)——跳过去不滚动定位到命中消息,预览里的定位能力在主窗口断掉。
- **修复方向**:1a 给会话头加可点(行为同标题命中)或视觉降级为纯分组标签;1b 让 `openInMainWindow` 接受 seq 并在打开后滚动定位(可复用预览的 `data-seq` 定位逻辑)。
- **回归验证**:搜索→点会话头跳会话;预览→在主窗口打开→主窗口滚动到命中消息并高亮。

### 文案两条(P3)

- **CH3-3/CH7-3**:`TransportError` 在 `transport/http.ts:226-238` 拼上 `[httpTransport] <status>:` 前缀,store/error bus 直接 `e.message` 上 toast,全链路无清洗。修复:展示层(`useErrorBus.extractErrorMessage`)剥前缀,或在 TransportError 上挂 `userMessage` 字段供 UI 取。
- **CH9-1**:`MemoryPreview.vue:479-485` footer 写着「详细规范见 docs/IMPLEMENTATION.md §4(B5 决策)」——写给仓库开发者的行文泄漏进产品 UI。修复:删掉或改成用户语言的功能说明。

---

## 3. 判定设计如此(B 类,关闭)

> 均已对照源码/数据确认是实现意图,除非产品主张变更,不作为缺陷重开。

| 编号 | 原判 | 实际判定 | 依据 |
|---|---|---|---|
| CH1-1 | P3 `/` 不重定向 | **撤销**(观察误差) | `router/index.ts:55-61` redirect 必然把 URL 换成 `/chat`(history 模式 pushState);截图无地址栏无法证实「URL 停留 /」 |
| CH1-2 | P3 `/pairing` 不重定向 | 设计 | `beforeEach` 首行 `if (to.name === "pairing") return true`(`router/index.ts:81`);注释写明 daemon 无 redeem 路由,锁进 pairing 是死路 |
| CH2-3 | P3 删当前会话后「新对话」不在列表 | 设计 | 会话懒创建(首条消息才 `create_session`,`chatSendActions.ts:280-287`);删除后空态与全新项目同构 |
| CH2-4 | — 同名会话重复 | 历史数据 | DB 实证 3 条同名「拉起一个子代理…600行」是真实独立记录(默认标题「新对话」+未改名所致),非前端重复渲染 |
| CH4-3 | P2 助手消息重发无响应 | 设计(残一小点) | 编辑/重发在助手消息上显式禁用 +「仅 user 消息」提示(`MessageActionsMenu.vue:131-141,243-264`,文件头注释"Re-firing an assistant message has no defined semantics"),父层再按 role 短路。禁用态视觉强度归 §5 |
| CH4-4 | P3 H2 与正文同字号 | 设计 | 2026-08-29 `ui-visual-polish r2` 注释明示标题层级压平(h2≈16.1px vs 正文 14px,靠字重/间距区分) |
| CH5-3 | P3 残留 /test-b3 | 环境数据 | `~/.config/everlasting/commands/test-b3.md` 真实存在,是本机用户自定义命令,非内置残留 |
| CH6-1 | P3 工具卡片默认折叠 | 设计 | 紧凑时间线取向,展开交互正常 |
| CH7-1 | P2 Edit 模式写文件不询问 | 设计 | 文档化分级策略:`permission.rs:275-294` 项目内写文件静默放行("the user trusts the agent to work in the repo");shell 三档(`shell_trust.rs`);kill list(`dangerous.rs`)连 Yolo 都拦。是否收紧属产品主张 |
| CH7-2 | P3 拒绝后显示「× error」 | 设计 | 拒绝统一走 error 通道,通用工具卡只有 running/done/error 三态;"user denied" 是后端合成 fallback(`ask.rs:557-565`) |
| CH8-1 | P2 子代理无提问工具 | 设计 | `tools_filter.rs:42-52` 注释明确:worker 阻塞等用户输入会挂死任务,由父代理转达——报告推测的机制正是设计 |
| CH10-1 | P3 设置中英混杂 | 设计 | `registry.ts:25` 注释「沿用原 tab 文案保持肌肉记忆」+ 双语 keywords 补偿搜索 |
| CH11-2 | P3 停用任务仍显示下次时间 | 设计 | design §2 明示灰显展示(后端 `scheduled_tasks.rs:270-279` 注释 + 前端 opacity 0.55) |
| CH13-观察 | — turn 跳号/耗时— | 设计 | 跳号=压缩摘要占号(`drive.rs:514-520`)+ softcap 询问占号(`chat_loop.rs:668-674`);耗时"—"=无 LLM 调用轮次的预期降级(`TurnCard.vue:99-124`) |
| CH14-2 | P3 超宽屏消息列不居中 | 设计 | IM 式版式:列表全宽 + 气泡 `min(75%,920px)` 限宽 + user 右/assistant 左(`MessageItem.vue:1163-1170`,08-29 刚调优行长) |

---

## 4. 人工复核项(C 类,均已关闭)

> 自动化手段测不可靠的项,交真人键盘/视觉复核。两项均已复核关闭,留档备查。

- **CH5-1 Shift+Enter 换行**:~~待复核~~ **✅ 已人工复核(2026-08-29,真实键盘):换行正常,关闭。** 原报告为自动化误报——输入框是 CodeMirror 6 contenteditable(`ChatInput.vue:27-32`),keymap 只绑 Enter(`chatInputCodeMirror.ts:771-821`),换行靠浏览器默认行为;非受信合成键盘事件不触发默认插入,自动化测不出是预期的。无需在 keymap 加 Shift-Enter 绑定。
- **CH14-1 Tab 焦点环弱**:~~待复核~~ **✅ 已人工复核(2026-08-29,真实键盘 Tab 走查):通过,关闭。** `:focus-visible` 基线存在(`style.css:319-353`,accent 20% alpha 3px ring),走查确认每处焦点均有可见指示,强度为「可见但不吵」的设计本意,无需调 alpha。

---

## 5. 功能建议池(D 类,非缺陷)

| 编号 | 建议 | 备注 |
|---|---|---|
| CH4-5 | 普通围栏代码块加块级复制按钮/语言标签 | 结构化卡片路径已有现成实现(`CodeBlockPrimitive.vue:32-56`),markdown 管线未接 |
| CH5-4 | 输入框草稿持久化(localStorage,按 session) | 现无任何草稿持久化,属新能力 |
| CH11-1 | 定时任务原生「一次性」档位 | 前后端六档对齐、都无 one-shot;现可用「结束条件=次数 1」近似(`compute.rs:44-60`) |
| CH12-2 | 搜索历史关键词卡片 | 未实现;`SearchHistoryCard.vue` 是 agent 的 search_history 工具卡,易混淆,别复用其名 |
| CH3-2 | 隐藏项目「重新打开」后下拉自动收起 | 小交互打磨 |
| CH7-4 | 放行管理「撤销」加确认 | 影响低 |
| CH8-2 | 提问卡滚动可见性 + 聊天回答与卡片提交并存提示 | 交互打磨 |
| CH4-3 | 助手消息菜单禁用项的灰态视觉加强 | 逻辑正确,纯视觉 |

---

## 6. 未覆盖项(下次测试补)

- 工具卡片图片结果展示;粘贴图片到输入区(自动化难构造图像剪贴板)。
- worktree 型 Worker 的分支徽章与合并控制。
- 真实远程设备配对(/pairing 提交属外向动作,未执行)。
