# N19 调研:mcode 语义吸收小件包 —— 现状缺口 / 改动面 / 取舍

> 调研日期:2026-09-29。对照材料:mcode 调研 [`docs/_history/research/minimax-code-survey.md`](../../../../docs/_history/research/minimax-code-survey.md) §3.2-3.5(pi-mono 补丁账本实锤教训);本项目代码实读(行号为 2026-09-29 main `427692f1` 现状)。
> 五条核验清单(BACKLOG 附录 C.2 N19 行):① edit_file fuzzy 匹配有无归一化破坏 CJK;② edit/diff 有无大小限界(Myers 二次方卡死风险);③ 后台 shell 停止是否直 SIGKILL(有无宽限);④ 宿主崩溃后后台进程组回收现状;⑤ 并行只读批取消时 tool_use/result 配对完整性。

## 结论速览

| 件 | 现状判定 | 增量价值 | 建议 |
|----|---------|---------|------|
| ① edit fuzzy CJK 保真 | **缺陷形态不存在**:无 fuzzy 匹配路径、零 Unicode 归一化,纯精确字节匹配 | 薄 | 1 条 CJK 往返保真测试锚(防未来引入 fuzzy/归一化时无声退化),或纯记注 |
| ② edit/diff 大小限界 | **三层结构天然规避**:edit_file 无 diff 计算;git diff = libgit2 C 实现 + untracked 64KiB cap;前端 word-diff 双守卫(N7 已做) | 薄 | 记注 tracked patch `to_buf` 无 per-file 上限的构建成本(C 实现、C6 截断兜输出面,低风险);不立项 |
| ③ 后台 shell SIGTERM 宽限 | **真实缺口(设计取舍可辩)**:前台/后台全部直 SIGKILL 进程组(RULE-E-002),trap 清理逻辑无机会执行 | 中 | `kill_and_collect` 两段式(SIGTERM → 宽限 → SIGKILL),前台后台同改,~30-50 行 + 时序测试 |
| ④ 父进程死亡守卫 | **真实缺口(比调研前预想实)**:daemon graceful shutdown 链**漏 kill_all**(Thin/sidecar/daemon.sh 场景后台 shell 全孤儿化);SIGKILL/崩溃面零守卫且超时计时器随进程死 | **实** | 立项主体:缺口 A = shutdown 链补 `kill_all`(~10-20 行);缺口 B(崩溃面 lease/PDEATHSIG)单列裁定 |
| ⑤ 并行批取消配对 | **形态存在但有下游自愈**:执行中途取消部分落库+剩余悬空,靠 wire 层每 turn 注入 synthetic 兜住(不 400);send 阶段取消已全量补齐 | 小 | finalize_turn 取消臂补齐差集(对齐 drive.rs:1993 语义),wire 自愈降级为真异常兜底,~20-40 行 |

**总评**:N19 立项范围应从"五件并重"收窄为「件④缺口A 为主体 + 件③ 两段式 + 件⑤ 差集补齐」三件单 PR;件① 测试锚顺手;件② 纯记注不实施。与 N12 同形:核验先行,一半小件判"已满足/不存在",真实缺口集中在一个此前无人注意的面上(daemon shutdown 链)。

---

## 件①:edit_file fuzzy CJK 保真 —— 缺陷形态不存在

### 现状(核验证据)

- `edit_file` 是**纯精确字节匹配**:`count_occurrences` 用 `str::match_indices`(`app/src-tauri/src/tools/edit_file.rs:251-256`),替换用 `current.replacen(old_string, new_string, 1)`(edit_file.rs:200-204)。Rust `str` 的 `match_indices` 按 UTF-8 字符边界匹配,不会撕裂多字节字符。
- **无 fuzzy 匹配路径**:模块头注释明说 0 匹配时不做 auto-strip-retry(claude-code 同款,edit_file.rs:7-11);`find_similar_lines`(edit_file.rs:291-316)只在 0 匹配时用 Jaccard 字符集合相似度挑 3 行做**错误提示**,不参与替换、不写文件。
- **零 Unicode 归一化**:全文件 grep 无 NFKC/NFC/`unicode-normalization` 引用;`find_similar_lines` 的 `split_whitespace` 归一仅作用于 hint 打分的临时副本。

### 与 pi-mono 教训的差距分析

pi-mono 的缺陷形态 = "fuzzy edit 路径对**整个文件**做 NFKC/ASCII 归一化,破坏智能引号/全角字符,修法 = 只替换映射回原文的精确 span + 保真守卫"。本仓库**没有 fuzzy 路径**,归一化破坏链的第一环就不存在;写回路径 `replacen` 是原字符串拼接,保真由构造保证。

代价面(设计取舍的另一侧):LLM 提供的 `old_string` 必须逐字节精确(含空白),不精确直接报错+hint。这是有意为之(claude-code 风格),对 CJK 用户反而是保真最优解。

### 改动面

1 条测试锚(可选,~15 行):全角/智能引号/混合 CJK 内容的 edit 往返保真单测(写入 → edit 邻近片段 → 断言全角字符字节不变)。价值 = 未来若引入 fuzzy 匹配或归一化(比如借鉴 mcode 的 fuzzy edit),该测试是第一道闸。**不引入任何 fuzzy 实现**——精确匹配是现状优点,不是缺口。

---

## 件②:edit/diff 大小限界 —— 三层结构天然规避

### 现状(核验证据)

1. **edit_file 工具本体无 diff 计算**:纯字符串替换(O(n)),mcode 的"20k 行 diff 167s"形态(归一化+diff 计算叠加)第一层就不存在。工具返回仅一行 summary(edit_file.rs:238-247),无 diff 正文。
2. **git diff 走 libgit2 C 实现**:`diff_tree_to_tree` / `diff_tree_to_workdir_with_index`(`app/src-tauri/src/git/diff.rs:104,199`);untracked 文件内容有 64KiB cap(`UNTRACKED_DIFF_CAP`,diff.rs:295 + :339-346 截断标记)。libgit2 的 xdiff 是 C 实现,与 mcode 的 TS 侧 diff 不可同日而语。
3. **前端 word-diff 双守卫(N7 已交付)**:`intraLineDiff.ts:29` `MAX_PAIR_LEN = 4000`(超长行对直接跳过行内高亮,原子 null 三路径)+ `DiffView.vue:65` `MAX_PAIR_RUNS = 200`。
4. **LLM 可见的输出面有 C6 截断**:工具输出超 64KiB 走 tool_output 契约模块截断/spill(08-30-c6),大 diff 不会整段进 context。

### 残余(如实记录,不立项)

tracked delta 的 `Patch::to_buf`(diff.rs:142-145)无 per-file 上限——超大 tracked 文件全重写时 patch 正文全量构建后才被下游(C6/前端)裁剪,构建期的 CPU/内存无闸。风险评级:低(C 实现性能 + 该路径消费面是 UI/命令而非 agent loop 热路径 + pathological 输入需要用户仓库里本就有巨大文件)。若未来 TurnCard「本轮 diff」在超大仓库出现可感知卡顿,再评估 per-file `to_buf` 早退;届时 mcode 的"20k 行 167s→0.2s"是反面前车之鉴,记注即可。

---

## 件③:后台 shell 停止直 SIGKILL —— 真实缺口,两段式收口

### 现状(核验证据)

所有终止路径**统一直 SIGKILL 整个进程组**:

- 前台 `shell` 工具:`kill_and_collect`(`app/src-tauri/src/tools/shell.rs:167-196`)→ `libc::kill(-pid_raw, libc::SIGKILL)`(shell.rs:176);取消臂(shell.rs:232)与超时臂(shell.rs:239-240)共用。
- 后台 shell:独立同构实现(`app/src-tauri/src/background_shell/in_memory.rs:1320-1345`,`libc::kill(-pid, SIGKILL)` 在 :1325);kill/timeout 两触发臂(in_memory.rs:1021-1034)共用。
- 触发面:`shell_kill` tool、超时、用户取消、`kill_all_for_session`(删 session)、`kill_all`(GUI 退出)。RULE-E-002 明文契约"SIGKILLs the entire process group"(`tools/shell_kill.rs:4-5`)。

即:**SIGTERM → 宽限期 → SIGKILL 的两段式不存在**。mcode(pi-mono 补丁)的教训 = 脚本的 trap 清理逻辑(临时文件回收、子进程优雅退出、锁释放)在 SIGKILL 下永远没有机会执行。 everlasting 用户面实例:agent 起的 dev server / test suite / build 被 kill 时,`node_modules/.cache`、临时文件、端口占用残留均靠外部超时清理,进程自身清理逻辑全部短路。

### 差距分析(取舍先行)

现状不是疏忽而是 RULE-E-002 的有意设计(SIGKILL 保证**收尾确定性**:不依赖子进程是否正确 trap,进程组必死,无僵尸/逃逸)。两段式引入的新变量:

1. **延迟**:SIGTERM 后需等宽限(典型 2-5s)才 SIGKILL,用户点「停止」/LLM 调 shell_kill 的体感变慢。
2. **不可杀窗口**:trap 逻辑本身卡死(常见于写得差的脚本)→ 宽限期满后 SIGKILL 兜底,最坏情形 = 现状 + 宽限期延迟。
3. **收益面**:trap 写得好的构建/测试工具(npm/cargo/make 均 trap SIGTERM)能清临时文件、断开端口、写完部分报告。

判定:收益真实(mcode 打补丁实证)且代价可控(宽限期是上限不是固定延迟——进程组退出即收)。收口后 RULE-E-002 语义从"必死"升级为"必死且先礼后兵"。

### 改动面

`kill_and_collect` 两段式(前台 shell.rs / 后台 in_memory.rs 各一份,或抽公共):`kill(-pid, SIGTERM)` → `tokio::time::timeout(GRACE_MS, child.wait())` → 超时再 `kill(-pid, SIGKILL)` + wait。~30-50 行 + 单测(trap 脚本宽限内退出=快路径;无 trap 脚本宽限满强杀;宽限期可 env 覆盖)。GRACE 缺省建议 3s(介于 mcode 与体感之间)。注意点:两处实现保持签名不变(调用方零改动);`ShellExitTrigger` 语义不变(killed 就是 killed,不区分是否经过宽限)。

---

## 件④:宿主崩溃后进程组回收 —— 真实缺口,且优雅路径也漏

### 现状(核验证据)

- **后台 shell spawn 无任何父进程死亡守卫**:`sh -c` + `process_group(0)` + `env_clear`(in_memory.rs:412-423),无 PDEATHSIG、无 pipe lease、无看门 launcher。max_runtime 超时是 **daemon 进程内的 tokio sleep**(in_memory.rs:1008)——daemon 死则计时器同死。
- **registry 纯内存**(`in_memory.rs`,模块头 mod.rs:14 "GUI-process in-memory impl"),daemon 重启即失忆:旧进程组既无人认领也无超时兜底,**永久孤儿跑到自然退出**。
- **唯一挂 `kill_all` 的位置是 GUI Full 模式的 `RunEvent::Exit`**(`app/src-tauri/src/lib.rs:584`)。
- **daemon graceful shutdown 链没有 `kill_all`**:`shutdown_signal`(daemon/server.rs:482-536)依次做 SSE 断开 → tunnel stop → scheduler cancel → disk governor cancel → agent loop drain,**无后台 shell 清杀**;daemon bin main 在 `serve_daemon` 返回后直接 exit(bin/everlasting-daemon.rs:242-247)。
- **PDEATHSIG 现有用法方向相反且只护 daemon 自身**:sidecar 模式 GUI 死 → daemon 收 SIGTERM(bin/everlasting-daemon.rs:83;sidecar.rs:41,172)→ 走 shutdown_signal graceful 链——而该链不杀后台 shell(上一条)。

### 差距分析(分场景)

| 场景 | 现状 | 判定 |
|------|------|------|
| GUI Full 模式正常退出 | RunEvent::Exit → kill_all | ✅ 覆盖 |
| Thin/sidecar 模式 GUI 退出 | daemon 收 SIGTERM → graceful 链**不杀 shell** → daemon exit → 孤儿 | ❌ **缺口 A(优雅路径漏杀,实锤)** |
| `daemon.sh stop`(SIGTERM)/ Ctrl+C | 同上 | ❌ 缺口 A |
| daemon SIGKILL / panic / 断电 | 无任何守卫;超时计时器随进程死 | ❌ 缺口 B(崩溃面) |
| GUI Full 模式 GUI 崩溃 | registry 在 GUI 进程内,同死 | ❌ 缺口 B |

缺口 A 是**一行级的漏挂**(shutdown 链已有五步,漏了第六步),修复成本极低、覆盖最高频场景(TUI/远程用户日常 `daemon.sh stop`);缺口 B 是 mcode 用 detached IPC lease + Unix sh gate + Windows launcher 解决的完整问题(宿主死了以后由第三方守卫收尸),在 everlasting 的等价物需要专门设计。

### 改动面

- **缺口 A(推荐立即收)**:`shutdown_signal` 步骤 2.5(agent loop drain 前后皆可,建议 drain 之后、进程 exit 前)补 `state.background_shells.kill_all().await`,与 GUI 模式的 Exit hook 对齐。~10-20 行 + 1 条 SIGTERM 集成测试(复用 server.rs 既有 SIGTERM 测试的进程级互斥锁样板,server.rs:538-549)。Windows 走 `child.kill()` 既有路径。
- **缺口 B(单列裁定,不并入本 PR)**:两个方向——
  - (a) **pre_exec PDEATHSIG**(轻):spawn 的 pre_exec closure 加 `prctl(PR_SET_PDEATHSIG, SIGKILL)`(纯 syscall,满足 sandbox pre_exec 的 W2 无 malloc 约束,可叠加)。局限:PDEATHSIG 只发给直接子进程(`sh -c`),孙进程孤儿仍存;且语义是"父**线程**"死亡——tokio worker 线程常驻使误杀窗口趋零,但语义要记入 spec;prctl 与 getppid 竞态需标准双检。
  - (b) **pipe lease / 看门 wrapper**(完整,mcode 同形):子进程持 daemon 的 pipe 读端,EOF 即 killpg 自杀。覆盖孙进程,但引入 wrapper 层(`sh -c` 外再套一层),与 sandbox pre_exec、env_clear、进程组语义全部交互,改动面大。
  - 建议:本 PR 只做缺口 A;缺口 B 以"崩溃面孤儿"单独记 BACKLOG 记注(发生频率 = daemon 崩溃频率,当前无崩溃收集(N11 未做)意味着真实频率未知,等 N11 或实际撞到再立项)。

---

## 件⑤:并行批取消配对 —— 形态存在,双层兜底已防 400,收口=对齐层次

### 现状(核验证据)

取消按发生阶段分两条路径,配对完整性**不对称**:

1. **send/流阶段取消**(工具未开始执行):drive.rs:1991-2019 对**全部** tool_calls 落库一条 synthetic is_error tool_result(`build_synthetic_tool_result_message`,helpers.rs:81-107)——这正是 mcode `terminateAgent` 补合成结果的语义,**已有**(还多覆盖了 had_error 路径,drive.rs:2049-2083)。
2. **工具执行阶段取消**:serial 循环 `if cancelled { break; }`(tools.rs:912-913,及 :978/:1050/:1185 同形)跳出,剩余未执行 tool_use **不补齐**;finalize_turn 取消臂只落库已完成的部分真 result(tools.rs:1926-1963 "persisted partial results")。L2 并行路径结构上每 task 必返回 slot(注释 tools.rs:464-470),配对天然完整——**缺口只在 serial 路径**。

**下游自愈兜底**:wire 层 `chat_request_to_wire`(llm/provider/wire/to_wire.rs:11-56)扫描 orphan tool_use → 每次请求构造时注入 synthetic is_error tool_result(08-06 群聊 speaker-desync 事故修复引入,事故根因是另一处 max_turns 逃逸)。所以下一 turn **不会** 400(Anthropic 2013 / OpenAI insufficient-tool)。

### 差距分析

功能面已兜住(自愈),语义面两层瑕疵:

1. **层次不对齐**:send 阶段取消在**落库层**补齐(历史自洽),执行阶段取消依赖**请求构造层**每 turn 动态补(不落库)——同一类事件两种处理层次。DB 历史不自洽的代价:wire 层 warn 日志每 turn 重刷一次("root cause is upstream" 自己都说这是防御);回放/导出/讨论库检索看到的历史缺一对;与「模型可见即已记录」不变量(C.3 记注)相悖。
2. **自愈的本职被稀释**:wire 自愈设计初衷是兜**真异常**(群聊 max_turns 逃逸那种不可枚举路径),现在 serial 取消这个**高频可枚举**路径也依赖它,真异常信号(该 warn 出现=有 bug)被噪音淹没。

### 改动面

finalize_turn 取消臂(tools.rs:1926-1963)补齐差集:取消时对 `tool_calls` 中**没有** result_block 的 id 追加 synthetic block(复用 `build_synthetic_tool_result_message` 的 block 构造,或对该函数加个"排除已配对"变体),随部分真 result 同一条 user 消息落库。需要把 `tool_calls` 传进 FinalizeFrame(现无此字段,加一个)。~20-40 行 + 单测(5 tool_use 中途取消 → 落库消息含 5 个 tool_result,2 真 3 synthetic;wire 层 warn 不再出现)。L2 并行路径不变;dispatch 并发批(subagent)不在本件范围(其取消走 `status=cancelled` 的 dispatch tool_result 语义,B6 已有契约)。

---

## 立项建议(回填 BACKLOG 用)

N19 实施范围收窄为三件单 PR:

1. **主体**:件④缺口 A —— daemon graceful shutdown 链补 `background_shells.kill_all()`(shutdown_signal 步骤 2.5),修 Thin/sidecar/daemon.sh 三场景的优雅退出孤儿;
2. **同 PR**:件③ —— `kill_and_collect` 两段式(SIGTERM → 3s 宽限 → SIGKILL),前台(shell.rs)/后台(in_memory.rs)同改;
3. **同 PR**:件⑤ —— finalize_turn 取消臂补齐未执行 tool_use 的 synthetic tool_result(对齐 drive.rs:1993 落库语义);
4. **顺手**:件① —— edit_file CJK 往返保真测试锚(~15 行)。

不实施、记注回 BACKLOG:件②(tracked patch `to_buf` 无 per-file 上限,低风险,撞到再评估);件④缺口 B(崩溃面 lease/PDEATHSIG,频率未知,等 N11 崩溃收集或实际撞到再立项)。

规模:小-中任务(单 PR 可收);改动面 = daemon/server.rs(shutdown 链)+ tools/shell.rs + background_shell/in_memory.rs(kill 两段式)+ chat_loop/tools.rs(finalize_turn)+ edit_file 测试锚;spec 落点 = sandbox-executor 无关,主要是 tool-contract(RULE-E-002 语义升级注记)+ LIFECYCLE 取消语义段。前置依赖:无。
