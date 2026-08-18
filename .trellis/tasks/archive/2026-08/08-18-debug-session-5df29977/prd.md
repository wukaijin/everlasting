# 排查最近对话 5df29977 的五项异常

> 任务来源:用户报告 2026-08-18 会话 `5df29977-2f4b-478e-ab22-01171fcd4aa2`(「看一下当前任务」,dev workflow 会话,08-46→09-29 UTC)期间出现 5 项异常。本任务负责逐项定位根因 + 修复 + 回归测试。DB 侧调查已完成,根因基本锁定,详见各问题小节「DB 证据」。

## Goal

修掉最近一次对话暴露的 5 个问题:

1. **edit_file 每次调用结果都带 memory** — P3/P5 pitfall pre-tool recall 把 Footnote 前置到**每个** edit_file 的 tool_result,即使该调用本身正常且与 pitfall 无关(仅 tool_name 命中即召回,command_pattern 未全量匹配)。
2. **loop 干预卡片答完后不消失** — 浮动 `<AskUserQuestionCard>` 提交成功后面板仍挂着,切走 session 再切回才消失;第二次干预才能正常点掉。
3. **shell tool 触发 21 次权限审批** — 同一 session 内 21 次 `tool_permission_ask`,其中 5 次 `permission_timeout`;用户希望放宽。
4. **勾选 dev workflow 后无法从 edit mode 切到 yolo** — 模式切换弹窗/确认流程在 workflow 会话下失效(疑似 modal z-index 被 workflow 相关覆盖层遮挡)。
5. **最后一次 loop 检测无相似操作仍触发** — seq 140 的 loop intervention 对应的调用与「最近的」操作并不相似。

## Requirements

### 问题 1:edit_file memory 注脚重复注入

**现象**:seq 116–120 起,每次 edit_file 的 tool_result 都以 `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —` 开头,携带两条 pitfall([Re-read file…] + [Always supply required path…]),即使 edit_file 已成功、参数完整。

**DB 证据**(session 5df29977):
- `autonomous_memories` 中两条 active pitfall 的 `tool_name='edit_file'`:
  - `01a01423-…` [Re-read file before retrying edit after on-disk change] `command_pattern='changed on disk since you last read it'` **hit_count=60**
  - `01a0142d-…` [Always supply required path in edit_file calls] `command_pattern='Missing required parameter: path'` **hit_count=15**
- `memory_token` 恒 2080(是 instructions 注入,非本问题);「⚠️ Memory」文案来自 `app/src-tauri/src/agent/permissions/check/pitfall.rs:119/397`(P3 `recall_pitfall_footnote` + P5 `recall_pitfall` 的 Footnote 分支)。
- 逐条 edit_file tool_result 都带注脚 → 说明 **P3/P5 recall 对每次 edit_file 调用都命中**,且命中后 `hit_count` 每次 +1(60 次 hit 佐证)。

**根因假设**:~~`find_pitfalls_by_trigger` 匹配口径太宽~~ → **已复验实锤(2026-08-18 二遍,见 research/code-re-evaluation.md §1)**,机制比假设更彻底:
1. `pitfall.rs extract_probe_args`:Path 类工具(edit_file)的 probe 恒为 `(None, path)` —— **从不提取 command_pattern**;
2. `db/memories/search.rs:398-405`:`command_pattern` 过滤只在 probe 为 `Some` 时生效,probe 为 `None` 时整段跳过;
3. 两条 pitfall 的 `path_globs` 均为空(DB 复核)→ NULL path_globs = path-agnostic = 恒命中。
三者叠加:每次 edit_file 必召回两条 + bump hit_count。

**⚠️ 设计矛盾(修复前需拍板)**:两条 pitfall 的 `command_pattern` 存的是**报错文本**(`changed on disk…` / `Missing required parameter: path`),而召回是 pre-tool、probe 的是 tool_input —— 输入永远不可能包含报错文本。「严格匹配 tool_input」对这类 pitfall 等于永不召回。二选一:
- **A(precision-first,小改)**:pitfall 带 command_pattern 而该 tool kind 的 probe 无 command → 不召回(参照同文件「path_globs 有值但 path 为 None → skip」先例);
- **B(补召回点)**:新增 post-tool 报错文本召回,把 command_pattern 对准 tool_result 的 error text。

**验收**:
- [ ] 正常参数、正常执行的 edit_file 的 tool_result **不再**带 `⚠️ Memory:` 注脚(方案 A)或仅在其 tool_result 报错文本真正包含 `command_pattern` 时召回(方案 B)。
- [ ] 修复后 `hit_count` 不再随每次 edit_file 无限增长(回归:单次正常 edit 不 bump 命中)。
- [ ] P3 footnote 与 P5 分档(SoftBlock 仅 verified+全量匹配)语义不被破坏。
- [ ] shell 类 pitfall(command_pattern 对准输入)的召回行为不回归。

### 问题 2:loop 干预卡片答完不消失

**现象**:第一次 loop intervention(seq 140)用户点「继续/终止」后面板不消失;切到别的 session 再切回才消失;第二次干预(本轮 seq 110)触发时才能正常点掉。用户描述「modal 始终挂着」。

**代码事实**(已定位):
- 浮动卡片 `ChatPanel.vue:987` `<div v-if="loopIntervention">` 由 `questionCards.pendingBySession[sid]` 驱动;`loopIntervention` computed 读取 `questionCardsStore.getPending(sid)`。
- 卡片复用 `<AskUserQuestionCard>`。其 `handleSubmit`/`handleSkip`(`AskUserQuestionCard.vue:285-325`)成功后仅翻转**本地** `localState` → `answered/cancelled`,并 `emit("answered"/"cancelled")`,**没有**调用 `questionCards.removePending(sessionId)`。父组件(`ChatPanel` 浮动卡片处)**也没有**监听该 emit 来移除 pending → `pendingBySession` 条目永不清理 → 浮动卡片 `v-if` 恒真。
- 对比:MessageItem 内联 ask 卡片同样依赖父级;真正的清理点只有 `resolveModeChange`/`resolveTaskStateTransition`(它们自己 `removePending`)与 session-switch 时的 `reconcilePendingInteractionFromBackend` 拉后端(后端 `QuestionStore.resolve` 已移除条目 → 返回 None → removePending)。所以「切 session 回来才消失」吻合。
- 后端 `resolve_tool_question`(`question.rs:114`)成功即 `question_store.resolve()`(移除 oneshot + 条目),**后端状态是对的**,只是前端 cache 没同步。
- **第二次能点掉已解释(2026-08-18 二遍复验)**:`streamEvents.ts reloadAfterFinalize`(每次流完成/done)会 pull 修正 → removePending。第一次干预(09:21:40)后 loop 又跑了 ~7 分钟不结束 → 无 done → 卡片一直挂 → 用户切 session 才清;第二次干预(09:28:18)后 run 于 09:29:10 结束 → done → 自动清除。**两次行为差异是运行时长巧合,不是不同代码路径**;`getPendingInteractionAction` 的调用点仅 ensureLoaded(切 session/建 session)+ reloadAfterFinalize,无提交侧清理。

**验收**:
- [ ] loop 干预浮动卡片提交/跳过成功后立即消失(前端在 resolve 成功后调用 `removePending(sessionId)`)。
- [ ] 不回归:内联 ask_user_question 卡片、mode_change 卡片、task_state_transition 卡片的行为不变。
- [ ] 后端已 resolve 的条目前端也能在**同一 session 不切换**时收敛(pull 修正点或 submit 后主动 removePending)。

### 问题 3:shell tool 21 次权限审批

**现象**:session 5df29977 中 21 次 `tool_permission_ask`(全部 `shell`),其中 `permission_granted` 16 次、`permission_timeout` 5 次(08:50、08:56、08:58、09:00、09:05 各一次,每 ~2 分钟一次)。用户希望放宽或解释原因。

**DB 证据**:
- 21 条 ask 的命令:git log/ls/wc/grep/cat/echo/git commit 等只读或常规命令;多命令用 `&&` 拼接(`cd … && git …`)或 heredoc 风格多行(`echo '=== … ==='\ngrep …`)。
- 权限表 `session_tool_permissions` 中该 session 只有 3 条 prefix 授权:`cd`(09:25:50)、`grep`(09:22:22)、`echo`(09:24:45)——**都是尾部才被授予**,且**没有 `git` 授权**(对比前一 session fec57568 有 `shell prefix git` 07-38)。
- 5 次 timeout 间隔约 2 分钟(08:50→08:56→08:58→09:00→09:05),且 08:54–09:05 恰是 LLM 在翻源码(grep tauri::command / builtin_tools / daemon routes)——像是用户在忙别的事没点,或前端弹窗被遮挡。

**根因假设** → **已复验修正(2026-08-18 二遍,见 research/code-re-evaluation.md §3)**。21 条 ask 全部 `mode:'edit'`,逐条归类:
1. **主因(16/21):`cd` 不在白名单**。`cd` 既不在 `READ_ONLY_WHITELIST` 也不在 `SIDE_EFFECT_WHITELIST`(shell_trust.rs 两表复核)→ classify_single 落兜底 **Ask** → 复合命令取 max → `cd … && git log`/`cd … && grep …` 整条 Ask → Edit 模式也弹窗。`git log`/`grep`/`ls` 本身是 ReadOnly,被 `cd` 拖下水。
2. **次因(5/21):引号内反引号**。命令含反引号(即使全在单引号里,如 grep pattern 带 markdown `` `0.80` ``)→ 整条 Ask(shell_trust 故意不做引号感知的 fail-safe)。09:22 起的 5 条全是这类。
3. **prefix grant 结构性救不了**:`permission.rs has_structural_metachar` 门(A2+ P1 安全设计)——复合命令**不享 prefix-grant 短路**(防 `ls` 授权放行 `ls; rm`);且 `first_token_for_allow_always` 只取首 token → 复合命令的弹窗只能授 `cd`,git 永远授不到。用户授的 cd/grep/echo 对后续复合命令全部无效。
4. 5 次 timeout = ask 超时 120s(audit 时间差恰 120s 佐证),用户未及时点;**无证据支持弹窗被遮挡**。

**验收**:
- [ ] `cd` 进 READ_ONLY_WHITELIST(只改子进程 cwd,零副作用)→ `cd X && <只读命令>` 复合不再弹窗(16/21 消除)。
- [ ] 引号内反引号:评估改引号感知(splitter 已有 4-state 引号状态机可复用)vs 维持 fail-safe;至少给出结论与依据。
- [ ] 5 次 timeout 原因已查清(120s 无人应答,agent 变体重试);如需放宽,给 ask 超时/重试策略一个结论(现状即答案,不强制改)。
- [ ] 复合命令 prefix 短路的 A2+ P1 安全语义**不得**放开;如要「授 git 前缀也能放行 `cd … && git …`」,需设计「逐段:cd=ReadOnly + git 段命中 grant」的复合授权口径,另行评审。

### 问题 4:dev workflow 会话下 edit→yolo 切换失效

**现象**:勾选 dev workflow(plugin `dev`)后,无法从 edit mode 切到 yolo mode(「无法从 edit mode 切换到 yolo mode」)。

**DB 证据**:
- 全库 audit 只有 2 条 `mode_changed`(2026-06-16 两条 plan 切换),**任何 workflow 会话都没有 mode_changed / yolo_entered 记录**;`sessions.mode` 里 10 个 workflow_enabled 会话 9 个是 edit、1 个 plan——**没有 session 切到过 yolo**(包括非 workflow 会话)。
- 会话 5df29977 的 audit 事件里也没有 `mode_changed` → 用户在这次会话里**根本没成功切过 mode**。
- 前端链路:`ModeSelect.vue` → `chatStore.requestSetMode(sid, 'yolo')` → 置 `pendingYoloConfirm=true` → `<YoloConfirmModal :open>`(ModeSelect.vue:316,普通 DOM 子节点,z-index 1200 的 fixed backdrop)。后端 `set_session_mode_internal` 只挡 root + 不拦 workflow。
- workflow 会话多一层 UI:`ChatPanel` 的 `ReviewMatrix`(仅 plugin=review)与 **ChecklistCard 浮动覆盖层**(`ChatPanel.vue:971`,z-index 50)在 workflow 会话出现;浮动卡片容器 `.chat-panel__loop-intervention` 也常驻层叠上下文。z-index 1200 的 yolo modal 正常应高于它们——但 **`YoloConfirmModal` 挂在 `ModeSelect` 内部**,而 `ModeSelect` 的 DOM 位于 `chat-panel` 内;若任一父级创建了层叠上下文(transform/filter/backdrop-filter),fixed 定位与 z-index 都会相对该父级解析 → 可能被遮挡或点击不到。

**根因假设**(~~modal 被 workflow 覆盖层遮挡~~)→ **已复验推翻(2026-08-18 二遍,见 research/code-re-evaluation.md §4)**,真因两条:

**A(数据 bug,顺带发现):INSERT 硬编码 `mode='chat'`**。`db/sessions/session_crud.rs:66` 的 INSERT 把 `'chat'` 写进 mode 槽(**b999803 workflow Step 2.2 加 plugin_name 时引入**,疑似与 session_type 的 'chat' 默认值混淆);返回的内存 struct 却是 `Mode::Edit`(104 行)——DB 与 struct 不一致。被掩盖:`migrations/schema.rs:497` 的 `UPDATE … SET mode='edit' WHERE mode='chat'` **每次 pool init 都跑**,只有「上次启动后新建」的 session 残留 'chat' —— 本次 DB 恰好只剩今早创建的 2 个 dev workflow 会话,制造了「与 workflow 相关」的假象(实为与「新创建+未重启」相关)。前端 ModeSelect.vue:104-108 把非 plan/yolo 一律显示为 Edit,用户以为自己在 edit mode。

**B(直接根因):root guard + 前端静默吞错**。本环境 app/daemon 以 root 运行;`set_session_mode_internal`(question.rs:384)对 `yolo + is_running_as_root()` 返回 Err("Cannot enable Yolo as root")——**按设计 yolo 在 root 环境永不可开**。而 `chatModeActions.ts confirmYolo` 的 catch 块只 `console.error`、无 toast → 用户确认后 modal 关闭、chip 不变、零反馈 → 体感「无法切换」。DB 指纹吻合:该 session 0 条 mode_changed、全库 0 个 yolo session;且 edit→plan 走同一 popover 未被报障,与「root guard 只挡 yolo」一致(若是 z-index 遮挡,plan 也应失败)。

**遗留**:z-index 遮挡降级为未证实(无证据亦无必要条件);YoloConfirmModal 未 Teleport 但 `.mode-select` 无 transform/filter,理论不构成 containing block trap,可在非 root 环境实测一次彻底排除。

**验收**:
- [ ] **修 INSERT**:create_session 写 `mode='edit'`(与内存 struct 一致);回归:新建 session 的 DB 行 mode='edit'。
- [ ] **修静默失败**:`confirmYolo` IPC 失败时 toast 后端错误文案(root 环境下用户能看到「Cannot enable Yolo as root」而非无反应)。
- [ ] 非 root 环境(或测试内绕过 root guard)下,dev workflow 会话 edit → Yolo 走完 confirm 流程后 `sessions.mode` 变为 yolo,audit 出现 `mode_changed` + `yolo_entered`。
- [ ] 非 workflow 会话回归:Yolo 切换行为不变;root guard 语义不变(仍拒绝 root 开 yolo)。
- [ ] (可选,实机一次)非 root 环境确认 YoloConfirmModal 正常弹出不被 checklist/loop 覆盖层遮挡,彻底关闭 z-index 假说。

### 问题 5:末尾 loop 干预对「不相似」操作误报

**现象**:seq 140 的 loop intervention(hit_count=3, 09:29:02)发生时,用户看记录并没有「相似操作」;但系统认为 3 连命中。

**DB 证据 + 代码机制**(已完全还原):
- loop 检测窗口是**最近 5 个 tool call 签名**(`drive.rs:1530-1535`,`loop_detection.rs` SOFT_WINDOW=5),`detect()` 对**窗口内所有 pairwise** 做 Jaccard,任一 pair >0.85 计数,≥2 对即 SoftLoop(`loop_detection.rs:159-183`)。
- `write_file` 的 signature 只含 path(`loop_detection.rs:198`:`"write_file" => format!("{}:{}", name, path())`)。**参数缺失的空调用 `write_file:{}` 的 signature 是 `write_file:`(path 取空串)**——3 次空调用签名完全相同。
- seq 129/131/133/135 连续 4 次 `write_file:{}`(LLM 漏传 path 参数,全部 `Missing required parameter: path` 失败)→ seq 136 hard 检测(hit 1,09:27:35,`loop detected: called write_file identical arguments 3 times`)。
- seq 137 成功写入 `progress.md`(签名 `write_file:{path}`)后,窗口 = [WFE, WFE, WFE, WFE(135), WFP(137)] → L1 尾部 run=1 不触发,但 **L2 里 4 个空 WFE 两两 Jaccard 1.0 → 6 对 > 2 → SoftLoop, hit 2**(turn_trace seq 138, verdict soft, hit_count=2)。
- seq 139 的 turn 调用 `request_task_state_transition`(工具本身与 write_file 完全不同),窗口变为 [WFE, WFE, WFE(135), WFP, RTS] → **残留的 3 个空 WFE 仍两两 1.0 → 3 对 → SoftLoop → hit 3 → 触发干预**(turn_trace seq 141 空、audit 09:29:02 loop_intervention asked)。

**根因**:① 空参数调用(签名退化为 `name:` 空串)与真实调用一样进窗口,3 个空调用互相 1.0;② `detect()` 的 SoftLoop 只看窗口内 pairwise 计数,**不要求任何一对与「当前最新调用」相似** → 早前残留的同类调用对在窗口内滞留 5 轮,把完全不同的新调用(切状态/写文件)误判为循环延续。

**验收**(2026-08-18 二遍微调 + **实现定稿**):
- [x] SoftLoop 增加 **recency-touch 门**:至少一个达标对的较新端落在窗口**最后两个位置**(`j >= n-2`),纯残影对(全部滞留窗口头部)不再触发;seq 139 场景复放用例(3 个残影空调用 + 2 个新调用)→ None。**实现于 `loop_detection.rs` detect() L2**。
- [x] ~~hard 后折叠窗口~~ **方案否决**(实现时推翻):折叠会让纯同一调用死循环在每次 hard 后窗口清空 → 下 turn 判 None → hit_count 清零 → **永远到不了 3-strike,worker loop_terminated 与干预全被架空**,违反「真实死循环仍被检出」。改为:recency 门单独承担消误报(hard 判定语义完全不动),见 drive.rs 内 design note。
- [x] 「继续」干预后 `loop_window.clear()`(连同既有 `loop_hit_count = 0`):用户明确给了新机会,残影不应在下一个 turn 立即重新计 hit(session 5df29977 在 09:21:40 continue 后 seq 112 立刻 soft hit 1 即此现象)。
- [x] 现有 loop_detection 单测全部保持通过 + 新增回归用例:`soft_loop_stale_pairs_without_recent_endpoint_is_none`(事故复放→None)、`soft_loop_latest_call_similar_to_history_still_fires`(最新调用相似→仍触发)。
- [x] 真实死循环(read_file 同 path 3 连 / shell 同 command 3 连 / 同签名 3 连失败)仍触发 HardLoop —— L1 语义零改动(hard_takes_precedence / three_identical_reads 等既有用例全绿)。

## Constraints

- 循环检测是**软提示**+ MAX_TURNS 硬兜底,不强制打断(§2.5.4 语义);问题 5 的修复不得把检测升级为硬打断。
- 权限(Yolo/root guard、Tier3 ask、kill list)语义不变;问题 3/4 是 UX/匹配粒度问题,不是安全降级。
- 前端修复遵循既有模式:`questionCards` store 是 pending 的唯一真源;IPC 成功后要 `removePending`(参照 `resolveModeChange` 的成功路径)。
- 排查优先于修复:问题 4 需先在 workflow 会话实机复现确认遮挡根因,再定修复方案。

## Notes

- 调查素材(本次已完成的 DB 取证)可落 `research/`;**2026-08-18 二遍复验结论落 `research/code-re-evaluation.md`(五项根因逐条对照代码,问题 4 z-index 假设被推翻、新发现 INSERT mode='chat' bug 与 root guard 静默失败)**;涉及文件:
  - `app/src-tauri/src/agent/loop_detection.rs`(窗口/Jaccard/签名)
  - `app/src-tauri/src/agent/chat_loop/drive.rs:1524-1600`(窗口维护 + hit 计数 + 干预)
  - `app/src-tauri/src/agent/permissions/check/pitfall.rs`(P3/P5 recall + Footnote)
  - `app/src/stores/questionCards.ts` / `app/src/components/chat/AskUserQuestionCard.vue` / `ChatPanel.vue:83-102,987-994`
  - `app/src/components/chat/ModeSelect.vue` / `YoloConfirmModal.vue` / `PluginSelect.vue`
  - `app/src-tauri/src/commands/question.rs`(`resolve_tool_question`)、`commands/sessions.rs`(workflow enable/plugin)、`agent/workflow/inject.rs`(build_workflow_ctx)
  - DB:session `5df29977-2f4b-478e-ab22-01171fcd4aa2` 的 messages/turn_trace/session_audit_events;`autonomous_memories` 两条 edit_file pitfall
