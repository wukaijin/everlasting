# 代码级重评估(2026-08-18,第二遍)

> 对 prd.md 五项根因假设逐条对照当前代码复验。结论:**问题 1/2/5 假设成立且进一步精确化;问题 3 假设方向对但主因锁定为 `cd` 分类;问题 4 的 z-index 假设被推翻**,真因是 root guard 静默失败 + 一个独立的 INSERT mode='chat' 数据 bug。

## 问题 1:pitfall 注脚恒注入 — 假设成立,机制精确化

PRD 假设「tool_name 命中即召回,command_pattern 未全量匹配」——**基本正确,但真正的机制更彻底**:

- `pitfall.rs:481-517 extract_probe_args`:Path 类工具(edit_file 属此列)的 probe 是 `(None, path)` —— **永远不提取 command_pattern**。
- `db/memories/search.rs:398-405`:`command_pattern` 过滤只在 probe 为 `Some` 时生效;probe 为 `None` 时**整段跳过**,不构成约束。
- DB 复核:两条 edit_file pitfall 的 `path_globs` 均为空 → NULL path_globs = path-agnostic = 恒命中(search.rs:425 注释明说 "always fires")。
- 三者叠加:**每次 edit_file 必然召回两条 pitfall + bump hit_count**(60/15 次佐证)。

**关键设计矛盾(PRD 未指出)**:两条 pitfall 的 `command_pattern` 存的是**报错文本**(`changed on disk since you last read it` / `Missing required parameter: path`),而召回是 **pre-tool、probe 的是 tool_input** —— 输入里永远不可能出现报错文本。即「让 command_pattern 严格匹配 tool_input」对这类 pitfall **永远匹配不上**,等于永不召回。修复需二选一:
1. precision-first(参照同文件 path_globs-有值-但-path-为-None → skip 的先例):pitfall 带 command_pattern 而 probe 无 command → 不召回;
2. 或新增 post-tool 报错文本召回点(把 command_pattern 对准 tool_result 的 error text)。

## 问题 2:loop 卡片答完不消失 — 假设成立,「第二次能点掉」已解释

- ChatPanel.vue:987-994 浮动卡片无 `@answered/@cancelled` 监听;AskUserQuestionCard.vue:285-325 submit/skip 成功后只翻 localState + emit,无 removePending。后端 resolve 正确(question.rs),纯前端 cache 滞留。✓
- **「第二次干预才能正常点掉」的解释(新发现)**:`streamEvents.ts:1064 reloadAfterFinalize`(每次流完成/done 触发)会调 `reconcilePendingInteractionFromBackend` → pull → removePending。
  - 第一次干预(09:21:40 answered)后 loop 继续跑了 7 分钟不停 → 无 done → 卡片一直挂着 → 用户切 session(ensureLoaded pull)才清掉;
  - 第二次干预(09:28:18 answered)后 run 于 09:29:10 结束 → done → reloadAfterFinalize → 卡片「自动」消失,看起来正常。
  - 即两次行为差异是**运行时长的巧合**,不是不同代码路径。修复(submit 成功即 removePending)后两者统一。

## 问题 3:21 次 shell 审批 — 假设方向对,主因修正为 `cd` 分类

21 条 ask 全部 `mode:'edit'`(audit payload 复核)。逐条归类:

| 主因 | 条数 | 机制 |
|---|---|---|
| `cd … && …` 复合命令 | 16 | `cd` 不在 READ_ONLY_WHITELIST 也不在 SIDE_EFFECT_WHITELIST(shell_trust.rs 两表复核)→ classify_single 落到兜底 **Ask** → 复合取 max → 整条 Ask → Edit 模式也弹窗。`git log`/`grep`/`ls` 本身都是 ReadOnly,被 `cd` 拖下水 |
| 引号内反引号 | 5 | shell_trust.rs 文档明说:命令含反引号/`$()` → Ask,**故意不做引号感知**(fail-safe)。09:22 起的 grep 命令 pattern 里含 markdown 反引号(如 `` `0.80` ``)→ 整条 Ask |

**prefix grant 为什么救不了(结构性的)**:
- `permission.rs:362 has_structural_metachar` 门:A2+ P1 规定**含 `|`/`&&`/`;` 的复合命令不享 prefix-grant 短路**(防止 `ls` 授权放行 `ls; rm -rf`)。所以用户点的「始终允许 cd/grep/echo」对后续复合命令**全部无效**。
- `first_token_for_allow_always` 只取首 token:复合命令的弹窗只能授 `cd`,git 永远不是首 token → 该 session 永远无法通过这些弹窗授到 git(前一 session 的 `git` 授权来自单段 git 命令)。
- 5 次 timeout = ask 超时 120s(audit 时间差 08:54:46→08:56:46 恰 120s),用户未及时点;无证据支持「弹窗被遮挡」。

**放宽结论**:白名单加 `cd`(只改子进程 cwd,零副作用)即可消掉 16/21;引号内反引号可改引号感知(spitter 本身已 4-state 引号感知,技术上可行,但需权衡 fail-safe 语义);复合命令 prefix 短路是安全设计,不建议放开,可考虑「复合命令每段都 ReadOnly/SideEffect 时静默放行」——但那已经是 classify 的行为,真正问题只在 `cd`。

## 问题 4:workflow 下 edit→yolo 失效 — **z-index 假设推翻**,真因两条

### 新发现 A(数据 bug):INSERT 硬编码 mode='chat'

`db/sessions/session_crud.rs:66`:

```sql
INSERT INTO sessions (…, color_tag, mode, workflow_enabled, plugin_name, session_type)
VALUES (…, NULL, 'chat', 0, 'dev', ?)
```

- **b999803(workflow Step 2.2,加 plugin_name 列时)**引入,把 'chat' 写进了 mode 槽(疑似与 session_type 的 'chat' 默认值混淆);返回的内存 struct 却是 `Mode::Edit`(104 行)——**DB 与 struct 不一致**。
- 被掩盖的原因:`db/migrations/schema.rs:497` 的 `UPDATE sessions SET mode='edit' WHERE mode='chat'` **每次 pool init 都跑**(幂等 scrub)→ 只有「上次启动之后新建」的 session 残留 'chat'。本次 DB 恰好只有今天 08:45/08:46 创建的 2 个 session 是 'chat',均为 dev workflow 会话 —— 这也是「问题似乎与 workflow 相关」的假象来源(实为与「新创建+未重启」相关)。
- 前端影响链:SessionSummary.mode='chat' → ModeSelect.vue:104-108 把非 plan/yolo 一律显示为 Edit(用户以为自己在 edit mode)→ `requestSetMode` 的 no-op 快路 `'chat' !== 目标` 不触发,功能上不直接挡切换。

### 新发现 B(直接根因):root guard + 前端静默吞错

- 本环境 app/daemon 以 root 运行(DB 文件属主 root、shell uid=0)。
- `commands/question.rs:384 set_session_mode_internal`:`new_mode == Yolo && is_running_as_root()` → Err("Cannot enable Yolo as root")(permissions.rs:64 `geteuid()==0`)。**本环境下 yolo 按设计永不可开**。
- `chatModeActions.ts:160 confirmYolo` catch 块只有 `console.error`,无 toast/modal 提示 → 用户点「我已知风险,启用 Yolo」后 modal 关闭、chip 仍是 Edit、**零反馈** → 体感「无法切换」。
- DB 证据吻合:该 session 0 条 mode_changed;全库 0 个 yolo session、0 条 yolo_entered。且若真因是 z-index 遮挡,edit→plan 也应同样失败(popover 同一菜单)——用户只报 yolo 失败,与 root guard 只挡 yolo 的指纹一致。
- PRD 里「后端 set_session_mode_internal 只挡 root 不拦 workflow」这句本身正确,但当时没把「本环境就是 root」和「前端吞错」连起来。

### 遗留待实机项

z-index 遮挡假说**降级为未证实**(无证据支持,也无必要条件);若要彻底排除,YoloConfirmModal 未用 Teleport、挂在 `.mode-select`(position:relative,无 transform/filter,理论不构成 containing block trap)——可在非 root 环境实测一次确认 modal 正常弹出。

## 问题 5:loop 误报 — 假设成立,全部代码点复核

- `loop_detection.rs:197` write_file 签名只含 path,参数缺失 → `write_file:` 空串。✓
- `detect()` L2(159-183)对窗口内**全部 pairwise** 做 Jaccard,不要求任何一对含最新调用。✓
- `drive.rs:1530-1535`:窗口在**执行前** push 全部 tool_calls → 失败调用(参数校验都没过)同样进窗口。✓
- seq 137/139 的窗口演化与 research/db-forensics.md 还原一致;hit 计数逻辑(1561-1565,非 loop turn 清零)确认。

**修复方向微调(PRD 验收第 2 条需再想)**:「失败调用不进窗口」会把 seq 136 的 HardLoop hint 也消掉——而那次 hard hint 恰恰促使 LLM 在 seq 137 改正(漏传 path 后补上)。连续同签名失败调用**是真 stuck,hard 检测是有用的**。更优口径:保留失败调用进窗口,但 (a) SoftLoop 要求相似对至少有一端是「最近 N 次内」的调用(或对 pair 按 recency 加权);或 (b) hard hint 已对签名 S 报过一次后,把窗口内 S 的重复项折叠/清出,避免 L2 反复收割同一批残影。seq 139 场景(残影对 + 新调用不同)在两种口径下都不触发,而 read_file 同 path 3 连仍检出。

## 五项结论速览

| # | PRD 假设 | 复验结论 |
|---|---|---|
| 1 | command_pattern 匹配口径太宽 | ✅ 成立但更彻底:Path 工具 probe 根本无 command_pattern + path_globs 空 → 恒命中;且 command_pattern 存的是报错文本,pre-tool input 匹配永远不可能命中(设计矛盾需拍板) |
| 2 | 前端不 removePending | ✅ 成立;「第二次能点掉」= reloadAfterFinalize 巧合,已解释 |
| 3 | 无 git 授权 + && 复合绕过 prefix | ⚠️ 部分成立;主因是 **`cd` 不在白名单 → Ask 档**(16/21),次因引号内反引号(5/21);prefix grant 对复合命令结构性无效(A2+ P1 安全门) |
| 4 | modal 被 workflow 覆盖层遮挡(z-index) | ❌ 推翻;真因:root guard 拒 yolo + confirmYolo 静默吞错;另发现 b999803 INSERT mode='chat' 数据 bug(被启动 scrub 掩盖) |
| 5 | 残留相似对 + 不含最新调用;空参数进窗口 | ✅ 全部成立;「失败调用不进窗口」的验收建议改为「hard 已报过的签名折叠出窗口」,避免把有用的 hard 自纠 hint 一起消掉 |

---

# 实现记录(2026-08-18 修复批次)

## 改动清单

| # | 文件 | 改动 |
|---|---|---|
| 1 | `db/memories/search.rs` | 两个 `find_pitfalls_by_trigger*` 的内存过滤:行有 `command_pattern` 而 probe 无 command → skip(precision-first,镜像 path_globs 先例);文档补 4-arm 语义 |
| 1 | `db/memories_tests/find_pitfalls.rs` | 新增 `find_pitfalls_command_pattern_row_without_probe_command_is_skipped`(含 all_status 变体 + 正向对照) |
| 2 | `components/chat/ChatPanel.vue` | 浮动干预卡片 `@answered/@cancelled` → `onLoopInterventionSettled` → `removePending(sid)` |
| 3 | `agent/permissions/shell_trust.rs` | `READ_ONLY_WHITELIST` 加 `cd`(只改子进程 cwd) |
| 3 | `agent/permissions/tests_shell_trust.rs` | `classify_sequence_compound_uses_segment_max` 更新:cd 复合→ReadOnly;补 `rm foo; ls`→Ask 保 max 语义覆盖 |
| 4 | `db/sessions/session_crud.rs` | INSERT mode 槽 `'chat'`→`'edit'` + 注释(b999803 混淆 session_type 默认值) |
| 4 | `db/sessions_tests/session_crud.rs` | 新增 `create_session_persists_edit_mode`(struct 与 DB 行双断言) |
| 4 | `stores/chatModeActions.ts` | `confirmYolo` catch → `showToast("Yolo 切换失败：…", "error", 5000)`(extractErrorMessage + 懒加载 projects store,镜像 questionCards 模式) |
| 4 | `stores/chatMode.test.ts` | 新增 root-guard 拒绝 → error toast 用例 |
| 5 | `agent/loop_detection.rs` | L2 **recency-touch 门**:达标对需 `j+2 >= n`(较新端落在最后两个位置);新增事故复放(→None)与正例(最新调用相似→仍触发)两个用例;模块文档同步 |
| 5 | `agent/chat_loop/drive.rs` | 「继续」干预分支 `loop_window.clear()`;hard 后折叠方案**否决**(design note 在 detect 调用点) |

## 设计决策:为什么否决「hard 后折叠窗口」

折叠(把 hard 报过的签名从窗口剔除)在推演中破坏 3-strike 升级:纯同一调用死循环每次 hard 后窗口清空 → 下 turn 判 None → `loop_hit_count` 归零 → 永远到不了 ≥3,worker 的 `loop_terminated` 直接break 和主 loop 干预全部失效(违反「真实死循环仍被检出」)。recency-touch 门单独即可消除 5df29977 的误报(单测 `soft_loop_stale_pairs_without_recent_endpoint_is_none` 在**无折叠**前提下复放事故窗口 → None),且 L1 hard 语义零改动。

## 测试结果

- 后端定向:loop_detection 36 全绿(含 2 新)、shell_trust 55 全绿、find_pitfalls 4 全绿(含 1 新)、session_crud 9 全绿(含 1 新)、p5_recall 6 + footnote 6 全绿、agent loop 集成 2 全绿。
- 后端全量 `cargo test --lib`:1821-1822 通过,2 失败(`tests_subagent::dispatch_main::guard_does_not_evict…` / `plan_mode…write_denied`,15s 截止时间类)。**干净树(stash 后)同样失败且更多(3 个)→ 既有并行负载 flaky,与本批改动无关**;两测试单独跑均绿(<1s)。
- 前端全量 `pnpm test`:73 文件 / 1100 用例全绿(含新 toast 用例)。

## 未做 / 后续

- **daemon 仍在跑旧二进制**(uptime ~5.5h):修复需重启 daemon/GUI 后生效;重启后可跑 `scripts/turn-smoke.sh` 做单轮 live 验证(本轮未重启用户环境,未做)。
- 问题 3 的「引号内反引号改引号感知」:评估后维持 fail-safe 现状(结论在 PRD),如要放宽另行立项。
- 问题 1 方案 B(post-tool 报错文本召回点):未实现,precision-first 已满足验收;两条存量 edit_file pitfall 此后不再被 pre-tool 召回(hit_count 停涨),若要「报错时才提醒」需补 B。

## Live 验证记录(2026-08-18 21:5x,daemon 重启 + release 重编译后)

daemon 二进制 21:52 重编(mtime 晚于全部源码改动)、PID 459947,经 `scripts/turn-smoke.sh` 四轮实测:

| 验证点 | 结果 |
|---|---|
| 问题 4a:新建 session mode | 3 个烟测 session 全部 `mode='edit'`(修复前 'chat');重启 scrub 已把存量 2 条 'chat' 洗净(现库 42 edit + 2 plan + 0 chat) |
| 问题 3:cd 复合命令 | 原样执行 `cd /tmp && ls \| head -3` 与 `cd … && git log` 形态 → audit 仅 `tool_allowed`/`tool_executed`,**零 `tool_permission_ask`**(修复前同款命令是 21 次弹窗的第一条,08:46:55) |
| 问题 1:edit_file 注脚 | 项目内健康编辑链(read_file → edit_file 成功)→ 全 session **0 条 `⚠️ Memory` 注脚**,两条 pitfall hit_count 不变(61/16)。修复前每次 edit_file 必带双坑注脚 + 双 bump |
| agent loop 新代码 | 4 轮 turn 全部干净跑完(recency 门在热路径),turn_trace 正常 |

**过程中两个非回归的干扰项(记录备查)**:
1. edit_file 写 /tmp(项目外)触发 Tier 4.1 越界写 ask —— 设计内行为,与本次修复无关;改用项目内文件后复验通过。
2. 一次 hit_count +1 来自 turn 起始的 **FTS 记忆召回**(memory_recall.rs,烟测消息含字面 "edit_file" 被 FTS 命中并 bump),不是 pre-tool 召回 —— 另一条路径,行为符合设计。

**未 live 覆盖(需 GUI/自然触发)**:问题 2 卡片即时消失(下次真实干预时观察)、问题 4b root 下点 Yolo 应见「Yolo 切换失败:Cannot enable Yolo as root」toast、问题 5 自然误报消失(单测+集成已覆盖)。

## 问题 4 用户实机确认(2026-08-18 22:2x)

用户在 GUI(dev workflow 开启的会话)点击 Yolo → 确认,现在**能看到错误 toast**(「Yolo 切换失败:Cannot enable Yolo as root」)。至此问题 4 闭环:症状与 dev workflow 无关(相关性来自 INSERT 'chat' bug 让新建 workflow 会话恰好是重灾区),真因 = root 环境下 yolo root guard 拒绝 + 旧前端静默吞错。REST 层复测同场:workflow=1 下 plan↔edit 切换 200 成功、yolo 返回规范错误 JSON——后端无任何 workflow 挡点。若需在本机使用 yolo:以非 root 用户跑 daemon/GUI(guard 按 `geteuid()==0` 判定,语义未动)。
