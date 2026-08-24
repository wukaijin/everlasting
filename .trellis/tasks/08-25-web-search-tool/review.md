# review.md — web_search 工具规划评审

> 评审日期:2026-08-25。对象:planning 期三件套(prd.md / design.md / implement.md)+ 两份 research。
> 方法:文档全部 file:line 主张与关键不变量逐一对照真实代码核验(核验时点 main @ 5a52d06,未含本任务任何代码——实施尚未开始)。

## 结论

**主干成立,修 3 个 P1 后再进实施。** 调研第一手(网络实测)、选型带数据、连锁面(stub/权限/名单)摸得全,绝大多数行号与不变量核对无误。P1 三项:① R6 漏 `.everlasting/` 项目层遮蔽(AC6 会假绿);② Tavily key 无清除路径 + auto 残留复活;③ design 契约核心留两个「?」未定稿。

---

## P1(实施前必须解决)

### P1-1 R6 漏 `.everlasting/workflow/` 项目层副本,AC6 验收假绿

- 事实:builtin workflow 是 `include_str!` **编译期 fallback**(`agent/workflow/builtin.rs:5`),运行时 loader **项目层优先**——查得到 `.everlasting/workflow/<name>/` 就不看 builtin。
- 本仓库自己有 git-tracked 的 `.everlasting/workflow/dev/agents/researcher.md`(与 builtin 内容已不同,diff 实证);`.everlasting/workflow/review/agents/reviewer.md` 同理。
- 后果:只改 `app/src-tauri/resources/builtin-workflow/**`(PRD R6 原文路径还少了 `app/src-tauri/` 前缀),本项目的 live researcher 仍拿旧工具表;grep 级验收照样全绿,**行为没变**。
- 要求:R6/AC6 增列两个 `.everlasting/workflow/**` 副本同步;或验收改为运行时断言 researcher 实际工具表含 web_search。

### P1-2 Tavily key 无清除路径,auto 选路让残留 key「复活」

- design §5:`set_web_search_config(provider, key: Option<String>)` None=不动 + 前端「留空 = 不改」→ UI 上**没有任何方式删除已存 key**。
- 连锁:用户切 `ddg` 想停用 Tavily,key 密文留在 `app_config`;日后切回 `auto` 时残留 key 让选路**静默走回 Tavily**(auto 按 key 有无静态选路),用户未必意识到。
- 要求:定义清除语义(空串 = 清除,或「清除 key」动作),PRD R2 / design §5 / 前端文案三处同步。

### P1-3 design 契约核心两个「?」未定稿

- **`#[async_trait?]`(design §2)**:仓内无 async-trait 依赖;`Provider` trait 先例是手写 `Pin<Box<dyn Stream>>`(`llm/provider/mod.rs:66`)。选路需要 dyn,而 Rust 1.75 原生 async fn in trait 不支持 dyn——§2 的签名画不下去。
  **建议:enum dispatch**。仅两 provider,`enum SearchBackend { Tavily(..), Ddg(..) }` + 一个 match 的 `async fn search`,免 dyn 免新依赖;「留口」= 加 variant;测试注 base_url 不受影响。比 trait + 手写 boxed future 更小。
- **CancellationToken 形参**:§2 签名带、§7 倾向不带,自相矛盾。**建议定死不带**——`tools/mod.rs:455` 外层通用 cancel 包装白得,reqwest future drop 即取消(§7 已自证)。

---

## P2(建议修,不阻塞)

1. **Goal 话术与 R2 冲突**:PRD Goal「零 IPC 框架、零 daemon 架构改动」vs R2 加 command + daemon route + transport 两方法 + Settings UI;research §6「IPC/daemon 零工作」也被 design §5 推翻且无标注。改成「工具执行面零 IPC(内联,search_history 先例);配置面走既有 app_config + 双形态 IPC」。
2. **daemon route 动词**:先例实为 **POST**(`/get_remote_config`、`/set_remote_config`,`daemon/routes/config.rs:74-75`),design 写「GET/PUT」——照先例走 POST,文档改口径。
3. **count clamp 公式与意图矛盾**:`unwrap_or(5).min(10)` 对 0 得 0(空结果),design §2 自己说「0/负数 → 默认 5」。应 `unwrap_or(5).clamp(1, 10)`;入参解析风格(手动 `as_u64` 取值)同时决定负数/类型错的行为,实现时收口在 execute。
4. **静态 token 线余量薄**:校准史实测 3903 / 线 3960 余 57;一个 stub 条目 ≈33 tok(10 个含 JSON 包装 330)→ 预计 ~3936,余 ~24。AC2「零平移」可达但 stub 描述必须一行极短;PRD 可预留「超线则按校准先例平移并记录」退路。另:3960 assert 实际在 `tools/stub.rs:363-365`(文档引 `:326` 是 docblock 区)。
5. **transport parity 测试**:接口加两方法后,按 `app/src/transport/transport-parity.test.ts` 的契约一致性精神应补新命令 parity 用例;implement.md WP2 未列,补一行。
6. **DDG UA 与自家实测反向**:实测默认 UA 200、Mozilla 202(research §1.1),design §3 却选 Mozilla(理由 IP 信誉主导、n=1 无统计力)。可接受,但 AC1 live 冒烟时 DDG 202 是预期内失败路径,别当回归卡住;UA 做成常量便于翻转。
7. **小项**:测试基线数字 PRD AC8「1926」vs implement「1925+1 flaky」,统一;`STUB_CANDIDATES`/`STUB_DESCRIPTIONS` 是定长 `[..; 10]` 数组(`tools/stub.rs:33-44`),加员同步改类型长度。

---

## 已核验无误的关键主张(背书清单)

| 主张 | 核验点 |
|---|---|
| stub 不变量有测试护住 | `tools/stub.rs:281-290` `candidates_disjoint_from_parallel_whitelist` |
| builtin_tools append-last 约定 | `tools/mod.rs:236-241`(search_history 注释明说 append-last 喂 prefix cache) |
| `execute_tool_inner` match + 通用 cancel 包装 | `tools/mod.rs:469` / `:450-467`(biased select) |
| 权限零改动路径(ToolKind::Other → Tier 5 静默放行) | `permissions/check/permission.rs:576-614`:不进 `classify_tool` 即 `Other` |
| `app_config` KV + AEAD(带 aad)签名 | `db/config.rs:34-53`;`crypto.rs:56/82` `encrypt/decrypt(master_key, plaintext, aad)` |
| tunnel config 双形态先例(command inner + daemon route) | `commands/config.rs:160-220`;`daemon/routes/config.rs:48-75` |
| execute 内读 provider 配置可行 | `ToolContext.db: SqlitePool`(`tools/mod.rs:362`),search_history 同款 |
| httpmock 0.7 在 / scraper 不在(手写解析前提成立) | `app/src-tauri/Cargo.toml:177`;全 Cargo 无 scraper |
| audit chip 加 `query` 分支必要 | `utils/audit.ts:535-565`:通用 fallback 落 JSON.stringify 截断,确实不可读 |
| researcher/reviewer frontmatter `tools:` 行号 | 两文件均在第 4 行(实际路径 `app/src-tauri/resources/builtin-workflow/`) |
| READONLY_TOOL_ALLOWLIST / GROUP_CHAT_RESEARCH_TOOLS / builtin researcher vec+prompt | `tools_filter.rs:128-135` / `group_chat_prompts.rs:195` / `registry.rs:82,94-100`,三处均穷举白名单,+1 即达 |
| `retry_open` 不可复用、借 `RetryPolicy::wait` 概念 | `llm/retry.rs:104` / `:186`(绑定 `&dyn Provider` 流) |
| edition 2021(P1-3 的前提) | `app/src-tauri/Cargo.toml:6` |

## 建议处理顺序

P1-1 改 PRD R6/AC6(增 `.everlasting` 副本同步)→ P1-2 补 key 清除语义 → P1-3 design §2 定稿(推荐 enum dispatch + 无 CancellationToken)→ P2 顺手带掉 → 按 implement.md WP1→WP4 开工。
