# Review — C6 大输出截断统一(planning 门评审)

> 评审对象:`prd.md` + `design.md`(2026-08-30,基于 main `7b1dc90`)。
> 任务状态:`planning`(`task.json.status`),尚未 `task.py start`。
> 证据核验:2026-08-30 对工作区实际代码逐一比对。

## 结论摘要

PRD / design 质量高:问题定位准确、三恢复模式 + 统一标记的契约抽象成立、spill
迁出项目树并捆绑 trusted pattern 的动机链完整。**design 中 18 处 file:line 证据
经逐条核验,15 处属实,3 处偏差**。核验中发现 **2 个 P1 级缺口**(design 未收录
shell 裸切片的 UTF-8 panic 隐患、未收录 background_shell 的第三份独立 spill
实现)与 **1 个必须 start 前定案的决策**(session_id 来源方案 (a)/(b) 悬而未决)。

**建议状态:需修订后放行**。修订量小(design 补 3 处证据 + 定 session_id 方案),
不推翻任何已定设计决策;修订后补 `implement.md` + 填充 jsonl 即可 start。

---

## 1. 证据核验结果(design §1 逐条)

| design 引用 | 实测 | 判定 |
|---|---|---|
| `shell.rs:74-83` 四常量 | `shell.rs:73-86` `MAX_OUTPUT_BYTES`/`DISK_SPILL_THRESHOLD`/`PREVIEW_BYTES`/`SPILL_DIR` 齐全 | ✅ |
| `shell.rs:492-527` 主路径 spill 流程 | `shell.rs:495-524` 一致(>30KB spill → 失败 warn 回退 inline) | ✅ |
| `shell.rs:530-560` `spill_to_disk` + `sweep_spilled_outputs` | `spill_to_disk` `shell.rs:533-543` 一致;但 sweep 函数**实为 `cleanup_outputs_dir`**(`shell.rs:557-583`) | ⚠️ 函数名不符 |
| `shell.rs:565-596` `head_tail_preview` + `truncate_output` "两套近似逻辑" | `shell.rs:568-596` 存在;**但两者均为裸字节切片,无 char-boundary 处理**(见 §2.1) | ⚠️ 漏记 panic 隐患 |
| `read_file.rs:38-45` 常量 | `read_file.rs:38-41` `MAX_OUTPUT_BYTES`/`TRUNCATE_HEAD` 一致 | ✅ |
| `read_file.rs:133-147,244-296` offset/limit + 头尾截断 | `read_file.rs:133-147` 参数解析,`truncate_output` `252-307` 一致;`truncate_full_output` `310` 独立存在 | ✅ |
| `grep.rs:26-27` `GREP_MAX_LINE_LENGTH=500` | `grep.rs:26-27` 一致 | ✅ |
| `grep.rs:279-300` `cap_line_lengths` `… <truncated>` 无指引 | `grep.rs:279-303` 一致 | ✅ |
| `grep.rs:157-263` `--line-number` + head_limit 提示 | `grep.rs:160-162` + 提示 `(hit head_limit of {}; narrow your pattern or raise the limit)` `253-260` 一致 | ✅ |
| `glob.rs:25,183-197` `MAX_RESULTS=100` + 收窄提示 | `glob.rs:25` + `183-197` 一致 | ✅ |
| `web_fetch.rs:79,93-97` 5MiB 硬错 + 100KB 截断 | `web_fetch.rs:79` `MAX_BODY_BYTES` + `93-97` `MAX_OUTPUT_BYTES`/head/tail 一致 | ✅ |
| `web_fetch.rs:646-660` 手写 char-boundary 修正 | `web_fetch.rs:646-661` 逐字节回退修正,注释自证 | ✅ |
| `mod.rs:385-426` `ToolContext.data_dir` 无 session_id | `mod.rs:387-424` 一致,无 session_id 字段 | ✅ |
| `attachments.rs:47-54` session-keyed 先例 | `attachments.rs:49-57` `attachments/<session_id>/` 一致 | ✅ |
| `sensitive.rs:68` static trusted pattern | `sensitive.rs:68` 一致 | ✅ |
| `sensitive.rs:89-96` 动态拼接 worktrees | `sensitive.rs:89-106` 一致 | ✅ |
| `permission.rs:295-308` read 族 trusted 放行 + ToolAllowed 审计 | `permission.rs:295-310` 一致;且 **Tier 2.5 sensitive deny(`88-131`)先于 Tier 4 allow**,AC4 deny 优先序成立 | ✅ |
| `read_file.rs:104-112` decouple | 该行段为 execute 签名/doc;decouple 历史注释位置略偏 | ⚠️ 行号微偏 |
| ROADMAP C6 来源 | `docs/ROADMAP.md:132`「C6 大输出截断统一 ⑩⑫ 边界处统一处理」 | ✅ |

另核验出 design 未收录的调用点:

- `background_shell/in_memory.rs:652,761` —— **第三份独立 `spill_to_disk`**(见 §2.2)。
- `commands/sessions.rs:373` —— `delete_session` 调 `cleanup_outputs_dir(cwd)`,为
  现状唯一 sweep 入口;`current_cwd` 为空时跳过(trim 判断)。
- `read_file.rs:310` `truncate_full_output` 为 `pub(crate)`,`agent::at_file` 复用。

---

## 2. 主要发现

### P1-1 shell 的 `truncate_output` / `head_tail_preview` 是裸字节切片,存在 UTF-8 panic 面(design 漏记)

`shell.rs:568-596` 两处切片(`&s[..head_end]`、`&s[tail_start..]`)均无
char-boundary 修正。Rust 切片到非边界索引必然 panic(语言保证)。对比同库其他
实现:`read_file` 用 `floor/ceil_char_boundary`(`read_file.rs:290-291`,
`truncate_full_output` 同),`web_fetch` 手写逐字节回退(`web_fetch.rs:650-658`),
`grep` 用 `chars().take(cap)` 天然安全——**唯独 shell 两条路径裸切**。

触发面:shell 输出 ≥30KB 且含多字节字符(CJK 路径/文件名、非 ASCII 日志)时,
spill 失败走 inline(`shell.rs:521`)或成功走 preview(`shell.rs:498`),
两处都可能 panic;spill 成功的 preview 是必经路径,即 >30KB 的中文输出**必然**
在 1KB 预览处 panic。`tests_shell.rs` 无任何 multibyte 用例(而 read_file 有
`truncate_full_output_multibyte_no_panic` 等,`read_file.rs:727,743`),此隐患从未
被测试覆盖。

影响:

1. 这是 R1「全库唯一一份 char-boundary 安全实现」最有力的动机,**比 web_fetch
   更严重**(主路径工具 + 无测试),但 design §1.1 只写「两套近似逻辑」,PRD 的
   UTF-8 叙事也聚焦 web_fetch「各自踩」,遗漏了最大受害者。
2. PRD C5「旧行为等价迁移」与统一 `head_tail_truncate` 存在张力:统一后 shell
   从 panic 变为安全截断,**行为不再「等价」——但这是修复不是回归**。C5 需显式
   注明「shell 多字节 panic 面消除属预期修复,不算语义回退」,否则实现者可能为
   了字面「等价」保留裸切片。

建议:design §1.1 补此证据(含 panic 面分析);PRD C5 加一句豁免说明。

### P1-2 `background_shell/in_memory.rs` 有第三份独立 spill 实现(design 证据表缺失)

`in_memory.rs:761` 自带 `spill_to_disk(cwd, stdout: &[u8], stderr: &[u8]) ->
Option<String>`,并**复制了三份常量** `PREVIEW_BYTES`/`DISK_SPILL_THRESHOLD`/
`SPILL_DIR`(`in_memory.rs:70-82`),stdout+stderr 合并写入同一
`.everlasting/outputs/` 目录。它不是「复用 shell 的实现」——design §2.3 表格的
「shell 家族 run_background_shell / shell_status 的 spill 复用点」措辞与现状不符。

影响:

1. R1 的迁移面实际是 **3 份 spill**(shell.rs + in_memory.rs + 未来 tool_output),
   不是 2 份;design §2.1「共享模块」的目标形态(单 `spill` 函数)要吃掉
   in_memory.rs:761 这份,并处理其 stdout+stderr 合并语义与 `Option<String>`
   返回(失败静默)差异。
2. 清理依赖同路径:`in_memory.rs:78` 注释自证「Same path … so cleanup is
   shared」——迁移后若只迁 shell 不迁 background_shell,新 sweep(按
   `<data_dir>/outputs/<session_id>` 删)将覆盖不到 background_shell 的旧路径
   落盘,双路径 sweep 的 legacy 清理逻辑会失配。
3. `spill_to_disk` 的路径返回用 `to_string_lossy()`,非 UTF-8 路径时与 shell 的
   `path.display()` 行为有细微差异,统一时一并收敛。

建议:design §1.1 证据表补 `in_memory.rs:70-82,652,761`;§2.3 迁移表把
run_background_shell / shell_status 列为「独立副本迁移」而非「复用点」;AC3 增加
background_shell spill 走新落点的验证项。

### P1-3 session_id 来源方案 (a)/(b) 未定案,start 前必须定

design §2.2 明确「两个方案定稿时择一」,倾向 (b)(`ToolContext` 增
`session_id: Option<String>`)。现状 `ToolContext` 构造点 10+(生产 + 各工具
test_ctx),方案 (b) 会波及全部构造点;方案 (a) 仿 read_file 加独立参数只动
execute 签名与 agent loop 调用点。

建议:定 (b) 的话,`ToolContext` 需配构造 helper 或 `..Default` 收敛,否则 test
构造点改动面会稀释本任务;若嫌波及大则退 (a)。无论哪个,AC3 需把
「session_id 来源」从 design 的 pending 项转为验收断言(例如:spill 目录名等于
当前 session id)。worker 路径的 session_id 语义(design §4 已列)也应在
implement.md 里写明「与 read_file 现状传法一致」。

### P2-1 sweep 函数名与代码不符 + stale PRD 引用

design §1.1 写 `sweep_spilled_outputs`,实际 `cleanup_outputs_dir`
(`shell.rs:557`)。另 `cleanup_outputs_dir` 的 doc 注释引用「PRD §R8」,是本任务
PRD 之外的旧编号,实现时应更新(避免误导)。

### P2-2 双路径 sweep 的边界细节需在 implement.md 写明

现状 sweep 入口唯一:`commands/sessions.rs:373` 按 `session.current_cwd` 删
`<cwd>/.everlasting/outputs`。迁移后需:新路径按 session_id 整目录删 +
legacy cwd 路径 best-effort 删。注意 `current_cwd` 为空(trim 判断)时两条路径
都应容错;legacy 路径若已被新 sweep 覆盖,需确认不会重复删或误删其他 session
(同 cwd 多 session 场景——现状已存在,迁移后仍按 cwd 删 legacy 即可,行为不变)。

### P3-1 planning 完成度:缺 `implement.md`,jsonl 未填充

workflow 明确 complex 任务 start 前需 `prd.md` + `design.md` + `implement.md`
且 implement/check.jsonl 各含至少一条真实条目。当前 jsonl 均为 seed 占位、
无 implement.md。放行条件见 §5。

### P3-2 行号微偏

`read_file.rs:104-112` 实际指向 execute 签名/doc,decouple 注释在别处;`shell.rs`
sweep 行号随函数名一并修正即可。均不影响结论。

---

## 3. 设计决策核对(design §3)

五条已定案决策均成立,与证据一致:

1. 契约统一而非数字统一 —— 成立,C2 与 per-tool 常量语义自洽。
2. spill 落 `<app_data_dir>/outputs/` 而非 home 点目录 —— 成立,与 attachments /
   worktrees / DB 单根约定同构;`sensitive.rs:75-79` 的三 OS 论证已核验。
3. 捆绑 trusted pattern —— 成立;权限层 `build_trusted_external_patterns`
   (`sensitive.rs:89-106`)已有 worktrees 动态段先例,`outputs/**` 同拼法即可;
   deny 优先序(Tier 2.5 > Tier 4)已核验,AC4 无回退风险。
4. web_fetch 落盘不重取 —— 成立,与 C3 一致。
5. token 治理分层 —— 成立,收口陈述不越界。

一个建议:决策 2 的论证可顺带引用 `sensitive.rs:75-79` 原文位置,design §3.2 已
隐含,不必展开。

---

## 4. 对 Acceptance Criteria 的检查

| AC | 判定 | 备注 |
|---|---|---|
| AC1 tool_output 单测(property + golden) | ✅ | 建议把 shell 裸切片 panic 场景作为首个 property 复现用例(见 P1-1) |
| AC2 迁移等价 + 既有测试全绿 | ⚠️ | 「等价」需按 P1-1 豁免 shell panic 修复;另 `tests_shell.rs` 无 multibyte 基线,「等价」无旧断言可对 |
| AC3 spill 新落点 + 整目录 sweep + legacy 清理 | ⚠️ | 需补 background_shell 迁移项(P1-2);session_id 来源定案后补断言(P1-3) |
| AC4 权限免弹窗 + deny 优先 | ✅ | Tier 2.5 > Tier 4 顺序已核验 |
| AC5 web_fetch 恢复链集成测试 | ✅ | |
| AC6 grep 行级 file:line 指引 | ✅ | rg `--line-number` 已带,信息现成 |
| AC7 全量测试 + clippy + fmt + turn-smoke | ✅ | turn-smoke 的 live 验证对 spill 路径变更必要 |
| AC8 spec 收编 `pattern-output-truncation` | ✅ | |

---

## 5. 放行条件(修订项汇总)

1. **design §1.1**:补 shell 裸切片 panic 证据(P1-1);补 `in_memory.rs` 第三份
   spill 副本证据(P1-2);修正 `sweep_spilled_outputs` → `cleanup_outputs_dir`
   (P2-1)。
2. **PRD C5**:注明「shell 多字节 panic 面消除属预期修复,不算语义回退」(P1-1)。
3. **session_id 方案定案**(P1-3,倾向 (b) + 构造 helper 收敛),并落到 AC3 断言。
4. 补 `implement.md`(PR 切分 PR1/PR2/PR3 已就绪,直接展开为有序清单),填充
   implement.jsonl / check.jsonl 真实条目(P3-1)。

完成上述后,可执行 `task.py start` 进入 Phase 2。
