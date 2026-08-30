# PRD — C6 大输出截断统一(截断契约 + spill 落点修正)

> 来源:[ROADMAP §2 第三档 C6](../../../../docs/ROADMAP.md)「大输出截断统一」。
> 设计结论定案于 2026-08-30 讨论会话(截断契约三模式 + spill 迁出项目树),证据 file:line 见 `design.md`。

## 背景与问题

大输出截断目前**散落在各工具各自实现**,上限口径、截断标记格式、恢复通路三者均不统一:

| 工具 | 截断机制 | LLM 能否看到完整输出 |
|------|---------|---------------------|
| `shell` | >30KB 落盘 spill(`<cwd>/.everlasting/outputs/`)+ 1KB 头尾预览;spill 失败回退 50KB 头尾截断 | ✅ 能(spill 路径 + read_file offset/limit) |
| `read_file` | 50KB(25KB+25KB)头尾截断;默认 2000 行 | ✅ 能(schema 明示 offset/limit,行号从真实行号起算) |
| `grep` | 每行 500 字符截断 + `head_limit` 截匹配数 | ⚠️ 半能(head_limit 可升/收窄;**单行截断后无恢复指引**) |
| `glob` | 100 条上限 | ⚠️ 只能收窄 pattern(无分页,属可接受兜底) |
| `web_fetch` | 5MiB body 硬报错;转换后 100KB 头尾截断 | ❌ **完全不能**(内容丢弃,无落盘无 range 参数)—— 最大缺口 |

三个层面的问题:

1. **恢复通路参差**:截断只保护 context,不给恢复手段等于死路。web_fetch 零恢复;
   grep 行级截断无指引。
2. **标记格式分裂**:至少 4 种(`<truncated: omitted N bytes>` / `… <truncated>` /
   `(...and N more matches; narrow your pattern)` / `Output saved to {path}...`),
   LLM 每种单独学。
3. **实现重复**:truncate_output 有三份(shell / read_file / web_fetch),spill 还有
   **第三份独立副本**(`background_shell/in_memory.rs` 自带 `spill_to_disk` + 整套
   复制常量,注释 "Matches crate::tools::shell's …" 人工对齐);UTF-8 char-boundary
   坑各自踩,web_fetch 靠注释手写 "Matches read_file's MAX_OUTPUT_BYTES" 对齐数字。
4. **shell 裸切片 UTF-8 panic(评审核出的 P1,已复核属实)**:shell 的
   preview / inline 截断是全库唯一无 char-boundary 处理的实现,违反已收编规则
   RULE-E-009(read_file / web_fetch / grep / subagent 均遵守);>30KB 多字节输出
   在 1KB 预览处高概率 panic,且 `tests_shell.rs` multibyte 用例为零,从未被覆盖。

**spill 落点问题**(独立于上三条,本任务一并修):spill 写在
`<validated_cwd>/.everlasting/outputs/`(shell.rs:83)——

- **Agent 自我污染**:spill 内容即本 session 工作输出,LLM 后续 grep 同关键词会命中
  自己的 spill 文件,`git status` 冒出不明 untracked 文件甚至被「清理」;
- **git 噪音**:app 无任何 .gitignore 注入逻辑,spill 文件暴露在 git status /
  `git add .` 误提交面;
- **语义混杂**:项目内 `.everlasting/` 是用户手写配置(`commands/`、project agents),
  运行时生成物不属于这里。

## 核心论点

**统一的对象是「截断契约」,不是「上限数字」。**

契约一句话:**每次截断必须自带一条恢复通路,且标记 machine-parsable、全工具同一格式。**

### 三种 sanctioned 恢复模式

| 模式 | 机制 | 适用 |
|------|------|------|
| **A 落盘 + 路径** | 全量写 `<app_data_dir>/outputs/<session_id>/`,标记给路径,LLM 用 read_file offset/limit 切片读 | 一次性执行、不可重放的结果(shell、web_fetch) |
| **B range 参数** | offset / limit / head_limit,标记里写明参数名 | 可重复执行的查询(read_file、grep) |
| **C 收窄提示** | 提示收窄查询(pattern narrowing) | 仅当结果可由更窄查询完整重放时兜底(glob,现状即合格样例) |

### 统一截断标记格式

```
<truncated: omitted N of M bytes | full output: {path} | recover: read_file offset/limit>
```

- `full output:` 段仅模式 A 出现;模式 B 换 `recover: re-run with offset/limit`;
  模式 C 换 `recover: narrow pattern`。
- 行数 / 条目数口径(grep、glob)允许 `omitted N of M matches` 变体,结构不变。

### token 治理分层(收口陈述)

**截断 = 工具侧单发防御(per-tool bytes);unified-context-budget 关卡⑤ = 轮级聚合防御。**
两层各管各的,C6 只动前者、只陈述后者。

## Requirements

- **R1 共享模块 `tool_output`**:`head_tail_truncate`(UTF-8 char-boundary 安全,
  全库唯一一份,以 RULE-E-009 为准绳)+ `spill`(字节入口)+ 统一标记生成器;
  shell / read_file / web_fetch / grep 全部迁移,`background_shell/in_memory.rs` 的
  **第三份独立 spill 副本**(自带常量,非复用点)一并吃掉。
- **R2 统一标记格式**:按上文格式落地,golden 测试钉死;各工具现存 4 种文案收敛为一种。
- **R3 spill 迁出项目树**:新落点 `<app_data_dir>/outputs/<session_id>/<uuid>.txt`
  (对齐 attachments 的 session-keyed 结构);**捆绑两件**——
  - 权限层 `build_trusted_external_patterns` 增 `<app_data_dir>/outputs/**`
    (read 族免 ask,信任推理同 worktrees:app 自建、agent 读、只读面);
  - sweep 改按 session 整目录删除;同时保留 legacy `<cwd>/.everlasting/outputs`
    best-effort 清理(升级前旧 session 的存量 spill 不悬空)。
- **R4 web_fetch 恢复通路**:转换后内容超 100KB 时 spill 到新落点,标记带路径;
  5MiB body 硬错(TooLarge)行为不变。
- **R5 grep 行级恢复指引**:行截断标记带「该行完整内容可 read_file 该文件该行」
  (rg 输出已带 `--line-number`,信息现成);head_limit 截断提示保留。
- **R6 glob 维持模式 C**:收窄提示原样保留,作为模式 C 的参照实现,不改。
- **R7 常量表 + spec 收编**:上限数字集中一张表,各自保留(语义不同:行 vs 条目 vs
  字节),但注明 token 预算依据;契约(三模式 + 标记格式 + 分层陈述 + spill 落点
  设计依据)收编 spec(按 pattern-* 惯例单档 `pattern-output-truncation`,
  归属定稿时按 spec index 定)。

## 约束

- C1. **wire 零变更**:纯后端工具层 + 权限 pattern 内部改动,无 IPC / HTTP 形状变化。
- C2. 上限数字**不强行拉齐**(30KB spill / 50KB inline / 100KB web / 500 chars / 100
  entries 语义各异);R7 只统一「出处 + 依据」,不改值。
- C3. web_fetch 不做流式 body 消费、不做 offset 重取(两次 fetch 内容可能漂移,
  落盘一次切片多次才诚实)。
- C4. C7(tools token 治理)与 unified-context-budget(关卡⑤)零改动,只写分层陈述。
- C5. 旧行为等价迁移:除标记文案统一外,shell / read_file 现有输出语义(头尾比例、
  预览大小、exit code 呈现)不变。**豁免**:shell 多字节裸切片 panic 面消除
  (RULE-E-009 违例修复,panic → 安全截断)属预期修复,不算语义回退;统一实现
  必须补 multibyte 回归测试(现状 tests_shell 零 multibyte 基线,无旧断言可对)。

## 非目标

- glob 分页(head_limit 式)——收窄是惯用法,不加分页。
- `turn_trace` omitted-bytes 观测计数——可选增强,若做另立小任务。
- attachments(图片)路径与清理——同构但零问题,不动。
- `.everlasting/` 项目内配置惯例(commands / agents)——不动。

## Acceptance Criteria

- [x] AC1. `tool_output` 模块单测:UTF-8 char-boundary 属性测试(**首个用例复现
  shell 裸切片 panic 场景**,RULE-E-009;含 CJK / emoji / U+FFFD 输入不 panic)、
  头尾尺寸、标记格式 golden(三模式变体全覆盖)。
- [x] AC2. 迁移等价:shell / read_file / web_fetch / grep 输出与旧实现逐项等价
  (除标记文案),既有工具测试全绿;等价基线以 ASCII 输出为准,shell multibyte
  由 panic 变安全截断属 C5 豁免,新增用例钉死(spill 成功 preview + 失败回退
  inline 两条)。
- [x] AC3. spill 新落点生效:新 spill 写 `<app_data_dir>/outputs/<session_id>/`
  (**断言:目录名 == 当前 session id**;session_id 走 execute 既有参数流——
  实现期发现 shell.rs:314-317 parity 参数已铺好,ToolContext 加字段方案取消);
  **background_shell 的 spill 同走新落点**;session 删除时整目录清理;legacy cwd
  路径 best-effort 清理仍执行(覆盖 shell + background_shell 两个旧写入方)。
- [x] AC4. 权限:`read_file` 读 spill 路径**免弹窗**(Tier 4 allow-list 命中,
  审计记 ToolAllowed);deny-list 优先级不回退(sensitive 路径仍先拒)。
- [x] AC5. web_fetch >100KB 内容可恢复:标记带绝对路径,read_file 该路径
  offset/limit 切片成功(集成测试走通恢复链)。
- [x] AC6. grep 行级截断标记含 file:line 恢复指引;head_limit 提示不变。
- [x] AC7. 全量 `cargo test -p everlasting --lib` + clippy + fmt 绿;
  `scripts/turn-smoke.sh` live 冒烟过(spill 路径变更经真实轮次验证)。
- [x] AC8. spec 收编 `pattern-output-truncation`(三模式契约 + 标记格式 +
  分层陈述 + spill 落点三依据),常量表入档。

## PR 切分建议

- **PR1**:`tool_output` 共享模块 + 四工具迁移 + spill 迁移(含权限 pattern、
  sweep 双路径)—— R1/R2/R3/R7 常量表。
- **PR2**:web_fetch 落盘恢复 —— R4。
- **PR3**:grep 行级指引 + spec 收编 —— R5/R6/R7 spec 档。
