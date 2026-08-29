# e2e 既有 2 红修复 + 纳入回归基线(RULE-TEST-003)

## 背景

2026-08-30 p3-debt-batch-cleanup 回归时发现(stash 对照验证非该批引入):干净 HEAD 上
`cargo test -p everlasting --test e2e` 即有 2 个失败,e2e 层处于无人跑的红态 —— CI
(ci.yml)只跑 `cargo test --lib`,e2e 从未进基线,坏也无人知。本任务修红 + 把 e2e
挂进 CI,闭合 DEBT.md §RULE-TEST-003。

## 根因(2026-08-30 实跑 + 源码考证,均已定位)

### e1a `chat_happy_path_httpmock`(e2e.rs:213,add_model 422≠200)

- 根因:commit `2b647c0`(B1 多模态 PR1)给 daemon 路由的 `AddModelRequest` 新增
  **必填** 字段 `supports_images: bool`(daemon/routes/providers.rs:110-119);
  e2e 的 `seed_catalog_and_session` fixture(e2e.rs:196-205)没带该字段 → serde
  反序列化拒收 → axum Json rejection 422。
- 定性:**fixture 过期,非生产回归**(前端 wire 早已带 supportsImages,lib 内
  add_model 相关单测全绿)。
- 修法:fixture 补 `"supports_images": false`。

### e1b `large_payload_skips_buffer_but_reaches_live`(e2e.rs:486,replay 非空)

- 根因:WP4(SSE 重连语义改造)后,「空 buffer + `Some(last)` 订阅」改为回
  **resync sentinel** 帧(指引前端走 snapshot),不再静默空回放 —— sse.rs 内联
  测试当时同步改为「先发小帧占 buffer 再证明大帧不入」的探测法(sse.rs:541-570
  注释自述),**e2e 里的同名副本未跟上,仍断言旧语义**。
- 定性:**镜像副本漂移,非生产回归**(内联版同名测试在 `--lib` 下绿,是权威副本)。
- 修法:**移除整个 e1b_sse_protocol 模块(e2e.rs:424-499),不是修单条**。理由:
  - 4 个测试全是纯 `SseRegistry` pub-API 单元副本(replay / sentinel / 大帧跳
    buffer / 首连不回放),零路由参与 —— sse.rs 内联套件 15 个用例全量覆盖同契约
    且更丰富(e2e 副本仅 4 个);
  - 本次红就是副本漂移的实证成本;留着其余 3 个在册副本,下次语义演进还会再漂;
  - e2e 层的独特价值是路由级集成(e1a chat / e1c snapshot / e1d health / e1e
    router smoke),registry 语义的权威 home 是 sse.rs。
  - 附带清理:e2e.rs:51 的
    `use everlasting_lib::daemon::sse::{SseRegistry, BUFFER_CAPACITY, LARGE_PAYLOAD_THRESHOLD}`
    仅 e1b 消费,整行删;文件头 `# Test groups` 的 E1b 条目改为指向 sse.rs 的一行说明。

## 范围

1. e2e.rs fixture 补 `supports_images`(e1a 修红)。
2. 删 e1b 模块 + 孤儿 import + 头注释改写(e1b 修红,去重防再漂)。
3. ci.yml Rust job 在 `cargo test --lib` 后补 `cargo test --test e2e`(e2e 是
   进程内 axum + httpmock 临时端口 + TempDir,无真 daemon / 无端口冲突,CI 可跑;
   sidecar 构建前置 `--lib` 已具备,复用同一编译产物)。
4. e2e 全量(`--test e2e` 用例)转绿验证;`--lib` 不回归。
5. DEBT.md §RULE-TEST-003 销账(删条目 + 尾表更新)。

## 非目标

- RULE-TEST-001(playwright 浏览器 runner 选型)—— 独立评估任务,保持 open。
- RULE-PERM-001(审计分页)—— 保持 open。
- sse.rs 内联套件本身不动(它是绿的权威副本)。
- 不给 e2e 层新增测试(只修红 + 进基线,扩覆盖另行立项)。

## 验收标准

- AC1:`cargo test -p everlasting --test e2e` 全绿(6 用例:e1a 2 + e1c 1 + e1d 2
  + e1e 1,删 e1b 4 后);`cargo test -p everlasting --lib` 全绿零回归。
- AC2:e2e.rs 内不再有与 sse.rs 重名/同义的 SseRegistry 单元副本;头注释准确反映现存分组。
- AC3:ci.yml Rust job 含 `--test e2e` 步骤;本地 `cargo clippy --test e2e` 零新增告警。
- AC4:DEBT.md 删 §RULE-TEST-003,尾表 P3 3→2、Total 3→2,与正文一致。
- AC5:生产代码零 diff(两处均为测试侧修复;若实施中发现 contrary 证据,停下重评)。
