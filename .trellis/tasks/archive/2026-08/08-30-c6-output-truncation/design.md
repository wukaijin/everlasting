# Design — C6 大输出截断统一

> 证据均于 2026-08-30 对 main(`7b1dc90`)实测核对。

## 1. 现状证据(file:line)

### 1.1 各工具截断实现

| 位置 | 内容 |
|------|------|
| `app/src-tauri/src/tools/shell.rs:74-83` | 常量:`MAX_OUTPUT_BYTES = 50KB`(inline 回退)/ `DISK_SPILL_THRESHOLD = 30KB` / `PREVIEW_BYTES = 1KB` / `SPILL_DIR = ".everlasting/outputs"` |
| `shell.rs:492-527` | 主路径:>30KB 先 `spill_to_disk`(成功 → 路径 + 头尾预览 + exit code 返回;失败 → warn 后走 inline 50KB 截断) |
| `shell.rs:530-560` | `spill_to_disk(cwd, contents)` 写 `<cwd>/.everlasting/outputs/<uuid>.txt`;清理函数实名 `cleanup_outputs_dir`(`shell.rs:550`,doc 引用旧「PRD §R8」编号,迁移时更新);**唯一调用入口** `commands/sessions.rs:371-373`(delete_session,`current_cwd` trim 空容错) |
| `shell.rs:565-596` | `head_tail_preview`(落盘预览,`...<truncated: omitted N bytes>...`)+ `truncate_output`(inline,`<truncated: omitted N bytes>`)—— 两套近似逻辑,且**均为裸字节切片,无 char-boundary 处理**(见 §1.1b panic 分析) |
| `background_shell/in_memory.rs:66-81` | **第三份独立副本**:常量 `PREVIEW_BYTES` / `DISK_SPILL_THRESHOLD` / `SPILL_DIR` 整套复制,注释 "Matches [`crate::tools::shell`]'s …" 人工对齐;`SPILL_DIR` 注释自证 "Same path … so cleanup is shared"(迁移只动 shell 会造成 sweep 失配) |
| `in_memory.rs:~653,757-780` | 独立 `spill_to_disk(cwd, stdout: &[u8], stderr: &[u8]) -> Option<String>`:**字节入口**(stdout/stderr 可能非 UTF-8,比 shell 的 `&str` 更忠实)、失败 warn + `None`、路径 `to_string_lossy()`;stdout+stderr 以 `[stderr]` 分隔符合并 —— 统一模块的签名须定字节/str 分层 |
| `read_file.rs:310` | `truncate_full_output` 为 `pub(crate)`,`agent::at_file`(at_file.rs:47,765)复用以保证 @文件注入与工具输出格式一致 —— 迁移后该出口语义必须保持 |
| `app/src-tauri/src/tools/read_file.rs:38-45` | `MAX_OUTPUT_BYTES = 50KB`,25KB+25KB 头尾布局 |
| `read_file.rs:133-147,244-296` | offset(1 行起)/ limit(默认 2000)切片 → 行号 → 若超 50KB 再头尾截断;`truncate_full_output` 与 `truncate_output` 又是独立实现 |
| `app/src-tauri/src/tools/grep.rs:26-27` | `GREP_MAX_LINE_LENGTH = 500` 每行截断 |
| `grep.rs:279-300` | `cap_line_lengths`:超 500 chars 的行截断后缀 `… <truncated>`(无恢复指引) |
| `grep.rs:157-263` | `--line-number` 已带;`head_limit` 截匹配数,提示 `hit head_limit of {}; narrow your pattern or raise the limit` |
| `app/src-tauri/src/tools/glob.rs:25,183-197` | `MAX_RESULTS = 100`;截断提示 `(...and N more matches; narrow your pattern to see them)` |
| `app/src-tauri/src/tools/web_fetch.rs:79,93-97` | `MAX_BODY_BYTES = 5MiB`(超限直接 `WebFetchError::TooLarge`)/ `MAX_OUTPUT_BYTES = 100KB`(转换后头尾截断;注释手写 "Matches read_file's …" 人工对齐) |
| `web_fetch.rs:646-660` | 自己的 `truncate_output`,注释自证 UTF-8 char-boundary 坑(CK/emoji/`U+FFFD` 切片 panic)——**每个副本都要单独踩** |

### 1.1b P1 级隐患:shell 裸切片 UTF-8 panic(评审 P1-1,已核验属实)

`shell.rs` 的 `head_tail_preview`(`:568-581`)与 `truncate_output`(`:583-596`)
均为 `&s[..head_end]` / `&s[tail_start..]` 裸字节切片,零 char-boundary 处理;
Rust 切片落在多字节序列中间**必然 panic**(语言保证)。同库对照:read_file 用
`floor/ceil_char_boundary`(read_file.rs:290-291,RULE-E-009)、web_fetch 手写
逐字节回退(web_fetch.rs:650-658)、grep 用 `chars().take(cap)` 天然安全、
subagent `truncate_summary.rs:390` 也遵守 RULE-E-009 —— shell 违反的是**已收编
spec 规则 RULE-E-009**,不只是「实现不一致」。(定案评审时判「全库唯一违例」;
**实现期修正**:`background_shell/in_memory.rs:543` 的镜像 `head_tail_preview`
同样裸切,违例是 shell + 其镜像两处,已随迁移一并消除。)

触发面:spill 成功路径的 preview 是 >30KB 输出的**必经点**
(`shell.rs:495-510`,`&s[..1024]` 在 1KB 处切),含多字节字符(CJK 路径/日志)
时高概率 panic(纯 CJK 连续段必然 —— 3 字节字符下 1024 落在序列中间);spill
失败回退的 inline 路径(`:519-524`,25KB 处切)同患。**测试零覆盖**:
`tests_shell.rs` multibyte 用例数为 0(read_file 侧有
`truncate_full_output_multibyte_no_panic`,read_file.rs:722-741)。

影响定性:这是 R1「全库唯一一份 char-boundary 安全实现」**最重的动机**(主路径
工具 + 无测试);PRD C5 已加豁免 —— 统一后 shell 从 panic 变安全截断**属预期
修复,不算语义回退**;AC1 的首个 property 用例即复现此场景。

### 1.2 spill 落点相关(迁移依据)

| 位置 | 内容 |
|------|------|
| `app/src-tauri/src/tools/mod.rs:385-426` | `ToolContext` 已有 `data_dir: PathBuf`(app 数据根,Tauri 解析;test 路径 tmpdir);**无 session_id 字段**(read_file 的 session_id 是独立参数,agent loop 传入) |
| `app/src-tauri/src/attachments.rs:47-54` | session-keyed 先例:`<app_data_dir>/attachments/<session_id>/<uuid>.<ext>` |
| `app/src-tauri/src/agent/permissions/sensitive.rs:68` | `STATIC_TRUSTED_EXTERNAL_PATTERNS = ["~/.config/everlasting/**"]` |
| `sensitive.rs:89-96` | `build_trusted_external_patterns(app_data_dir)` 启动期拼接 `+ <app_data_dir>/worktrees/**`;注释明确为何必须动态 app_data_dir(三 OS 布局 + bundle id 差异) |
| `agent/permissions/check/permission.rs:295-308` | read 族(`read_file|grep|glob|list_dir`)项目外读的放行分支:`is_trusted_external` 命中 → silent Allow + ToolAllowed 审计;否则 ask_path 弹窗 |
| `read_file.rs:104-112` | read 侧 2026-07-01 decouple:工具层不再 assert_within_root,项目外读全交给权限层(Tier 2.5 deny → Tier 4 trusted → ask) |

**迁移的动机链**:cwd 内 spill 的自我污染 / git 噪音 / 语义混杂(prd.md 背景节);
**当初放 cwd 的隐含原因**:项目内读静默放行,零权限摩擦 → 所以迁出**必须**捆绑
trusted pattern,否则恢复通路每读一次弹一次窗(见 §3)。

## 2. 目标形态

### 2.1 `tool_output` 模块(新,`app/src-tauri/src/tools/tool_output.rs`)

```rust
// 上限常量表(集中出处,值不变 —— 约束 C2)
pub(crate) const INLINE_CAP_BYTES: usize;      // 50KB  (read_file / shell inline)
pub(crate) const SPILL_THRESHOLD_BYTES: usize; // 30KB  (shell)
pub(crate) const SPILL_PREVIEW_BYTES: usize;   // 1KB   (shell 落盘预览)
pub(crate) const WEB_INLINE_CAP_BYTES: usize;  // 100KB (web_fetch)

// 三件套
pub(crate) fn head_tail_truncate(s: &str, head: usize, tail: usize) -> String;
    // UTF-8 char-boundary 安全;唯一实现,property 测试钉死
pub(crate) fn spill(data_dir: &Path, session_id: &str, contents: &[u8]) -> io::Result<PathBuf>;
    // 字节入口(最忠实:background_shell 的 stdout/stderr 可能非 UTF-8,
    // 不做有损转换落盘);str 调用方传 .as_bytes()。
    // 写 <data_dir>/outputs/<session_id>/<uuid>.txt
pub(crate) enum Recovery { Spill { path: PathBuf }, RangeParams { hint: String }, NarrowPattern }
pub(crate) fn truncation_marker(omitted: usize, total: usize, unit: Unit, recovery: &Recovery) -> String;
    // 统一格式,见 prd.md;golden 测试钉死
```

### 2.2 新落点与生命周期

```
<app_data_dir>/outputs/<session_id>/<uuid>.txt     # 新 spill(对齐 attachments 结构)
```

- **权限**:`build_trusted_external_patterns` 增 `{app_data_dir}/outputs/**`
  (动态段,同 worktrees 拼法);read 族免 ask,审计照记 ToolAllowed。
- **sweep**:session 删除时 `remove_dir_all(<data_dir>/outputs/<session_id>)`(整目录,
  比 cwd 递归删简单);**同时保留 legacy** `<cwd>/.everlasting/outputs` best-effort
  清理,防升级前旧 session 存量悬空。
- **session_id 来源(实现期改定:零改动,用现状参数流)**:定案评审时选了方案
  (b)(ToolContext 加字段);动手时发现**管道早已铺好**——`shell::execute` 签名
  本就带 `_session_id: Option<&str>`(shell.rs:314-317,doc 自述 "for parity with
  the other tools",一直未用),background_shell 的 registry 按
  `(session_id, shell_id)` 键查(in_memory.rs spill 调用点 session_id 就在作用域)。
  方案 (b) 的字段将无消费方(纯冗余),**取消**;两处 spill 直接消费既有参数,
  `None` 时退 `_no_session` 目录(sweep 按目录删不受影响)。AC3 断言不变
  (目录名 == session id)。

### 2.3 各工具迁移点

| 工具 | 改动 |
|------|------|
| shell | `spill_to_disk(cwd,…)` → `spill(data_dir, session_id, …)`;preview/mark 走统一生成器;inline 回退走 `head_tail_truncate`;**裸切片 panic 面随之消除(§1.1b,C5 豁免)**;补 multibyte 用例 |
| background_shell / in_memory.rs | **独立副本迁移(非复用点,评审 P1-2)**:删其私有 `spill_to_disk` + 三常量,stdout/stderr 字节直传 `tool_output::spill`;`[stderr]` 合并语义保留(在调用侧拼或作为 spill 参数);`Option<String>` 失败静默语义保留(调用方 warn);run_background_shell / shell_status 工具层只消费 in_memory 的返回,随动 |
| read_file | 两个 truncate 函数合并到共享实现;截断标记换统一格式 + `recover: re-run with offset/limit` |
| web_fetch(PR2) | 转换后 >100KB:spill + `full output: {path}` 标记;`TooLarge` 5MiB 硬错不变 |
| grep | 行截断标记带 `read_file {file} offset {line}` 指引;head_limit 提示并入统一格式(模式 B) |
| glob | 不动(模式 C 参照实现,spec 里引为样例) |

## 3. 关键设计决策(已定案)

1. **统一的是契约不是数字**:三恢复模式(A 落盘 / B range / C 收窄)+ 统一标记;
   数字留 per-tool(语义不同),但集中一张常量表 + token 依据。
2. **spill 迁 `<app_data_dir>/outputs/` 而非 `~/.everlasting/`**:app 单根约定
   (DB / attachments / worktrees 都在 app_data_dir),session-keyed 与 attachments
   同构;home 点目录是前 XDG 风格且 macOS/Windows 无对应物(sensitive.rs:75-79 论证)。
3. **迁移捆绑 trusted pattern(缺了就是倒退)**:不加 `<app_data_dir>/outputs/**`,
   read_file 恢复 spill 每次走 ask_path 弹窗,恢复通路名存实亡。
4. **web_fetch 走落盘不走重取**:两次 fetch 内容可能漂移(attribution 时间戳会诚实
   暴露这点,但恢复语义已经破了);落盘一次、切片多次。
5. **token 治理分层陈述**:截断(工具侧单发)vs 关卡⑤ budget gate(轮级聚合),
   C6 不动后者,只把分工写进 spec 收口。
6. **统一实现以 RULE-E-009 为准绳**(评审后补):char-boundary 安全是已收编规则,
   shell 是全库唯一违例(§1.1b);统一后 panic→安全截断属预期修复(PRD C5 豁免),
   实现者不得为字面「等价」保留裸切片。
7. **spill 字节入口 + session_id 走 ToolContext**(评审后定):见 §2.1 / §2.2,
   background_shell 的 `&[u8]` 入口是更忠实的形态,`&str` 调用方适配而非反过来。

## 4. 风险与边界

- **路径长度/权限差异**:app_data_dir 在三 OS 布局不同,标记里给绝对路径
  (现状 shell 已是绝对路径 `path.display()`,无回归)。
- **旧 spill 悬空**:legacy cwd 双路径 sweep 兜住;不做数据迁移(运行时产物,不值得)。
- **daemon / GUI 双端**:spill 在工具层(进程内),与 transport 无关,wire 零变更。
- **worker / subagent**:worker 有独立 ctx,`session_id` 若取 worker run 所属 session
  即可正常 keying;实现时确认 worker 路径的 session_id 语义(read_file 现状怎么传就怎么用)。
