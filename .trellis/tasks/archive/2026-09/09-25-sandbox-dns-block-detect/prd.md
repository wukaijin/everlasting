# classify_block 补 DNS 失败特征族——点亮网络拦截弹卡/grant 全链

## Goal

让「沙箱内短命出网命令」（git pull/fetch、curl 等）被 seccomp 断网拦截时，
既有识别→引导→升级卡→durable prefix grant 全链自动点亮。当前 DNS 失败
形态（`Could not resolve host` / `Temporary failure in name resolution`）
不在 `classify_block` 特征集里，整链哑火，模型只能自行诊断并放弃
（实证：jjh-mono session `ce51a3ba`，2026-09-25，`git pull` 拉不到远程）。

## Background（已确认事实）

- seccomp INET filter 拦 `socket(AF_INET/AF_INET6)` 创建（EPERM，seccomp.rs:78）；
  DNS 查询也要建 socket → resolver 不可达 → curl/git 表面化为
  `Could not resolve host: <host>`，glibc 表面化为
  `Temporary failure in name resolution`。EPERM 字符串被 resolver 吞掉，
  现有特征（listen/ONP 形态）一个不匹配。
- 宿主网络正常（实证 session 的域名宿主侧可解析、可连通），失败归因沙箱无歧义。
- 这是 spec sandbox-executor.md §12（listen 识别缺口，09-21/09-22 已修）的
  姊妹缺口，修复纪律沿用 §12.2 先例：强特征两流喂、宁缺勿滥、R9 归因合取、
  live 形态逐字节入测试锚、卡证据行 MARKERS 同步补锚。
- 升级闭环/grant 机制已建成（§10/§14）：classify 命中 Network → 有 grant
  免沙箱重跑 / 无 grant 弹卡（AllowOnce / AllowAlways→durable prefix grant）。
  classify 单源修复即全链点亮（前台 guidance + P3c 卡 + P3d 后台 offer）。

详细证据与文件锚见 `research/dns-block-evidence.md`。

## Requirements

- **R1 特征集扩展**：`classify_block` 的网络识别补 DNS 失败特征族——
  `could not resolve host`（ci）与 `temporary failure in name resolution`（ci），
  双流（stderr/stdout）扫描，与既有 listen 强特征同纪律。不收
  `Name or service not known`（健康宿主真 NXDOMAIN 同文案，误归因面大）。
- **R2 R9 归因合取不变**：DNS 族与 listen 族同样仅当 `net == InetBlock`
  （spawn 实装了 seccomp INET filter）才归类 Network；BindOnly spawn 上的
  DNS 失败不得归因沙箱。
- **R3 卡证据行补锚**：`escalation.rs` 的 `stderr_evidence_line` MARKERS 与
  `stdout_net_evidence_line` 补 DNS 族锚，弹卡证据行取到 `fatal: unable to
  access ...` 实证行而非任意尾行。
- **R4 引导文案补 grant 通道**：`failure_guidance_for_kind` Network 档
  （Edit）文案补 durable prefix grant 出路：短命网络命令（点名 git pull/fetch）
  批准卡「总是允许」后同前缀免沙箱启动；明示 grant 不覆盖复合命令，须单命令。
  Plan 档文案不动（Plan 不弹卡不豁免，§14 语义）。
- **R5 测试锚**（tests_sandbox.rs + escalation 侧，先例
  `classify_block_reads_stdout_for_listen_denials` 族）：
  - DNS 形态双流命中：jjh-mono 实证行逐字节入锚（stdout 侧）+ stderr 侧
    同族（无 `2>&1` 的 git pull）；
  - `Temporary failure in name resolution` 形态命中；
  - 宁缺勿滥锚：`Name or service not known` 不触发；
  - R9 锚：非 InetBlock spawn 上 DNS 文案不归类 Network；
  - 文案锚：Network Edit 档 guidance 含 grant 出路表述（pin 关键短语）。

## Acceptance Criteria

- [ ] 单命令 `git pull`（无 `2>&1`，DNS 失败落 stderr，exit≠0）在 Block 档
  sandboxed spawn 后：guidance 行追加、升级卡弹出（无 grant 时）、卡证据行
  为 `fatal: ... Could not resolve host ...` 实证行——单元测试锚覆盖
  classify/evidence/guidance 三点。
- [ ] `git pull ... 2>&1` 形态（DNS 失败落 stdout，整体 exit≠0）同样命中
  （实证 session 逐字节形态入锚）。
- [ ] durable grant 写入后（`git pull` 前缀），同前缀单命令在沙箱档
  免沙箱启动不再被拦（既有 §14 机制，回归确认不被本改动破坏）。
- [ ] BindOnly spawn 上的 DNS 失败文案不归类 Network（R9 回归锚）。
- [ ] `cargo test -p everlasting --lib` 全绿（含既有 listen 族锚不回归）。

## Out of Scope

- exit-0 复合命令掩蔽：`exit_code != 0` 闸保持不变（宁缺勿滥；listen 族
  同边界），文案引导单命令即为出路，不改闸。
- 结构性网络放行（spec §12.3.A `readwrite_net` 档 / connect 端口白名单）：
  用户已决策走轻量通道，如未来成主流诉求另行立项。
- seccomp/landlock 执法面任何改动：零改动，纯识别+文案层。
- Plan 档网络引导文案：不动。

## Technical Notes

- 改动面：`sandbox/mod.rs`（classify_block / stream_smells_net_block /
  failure_guidance_for_kind）、`agent/permissions/escalation.rs`（两处
  evidence line MARKERS）、`sandbox/tests_sandbox.rs`（+escalation 侧测试）。
- 误报面已评估（research 文件）：grep/cat 日志自引用被 exit≠0 闸消掉；
  残余误报代价 = 一张可拒的卡 + 一行引导，与 Write 族既有容忍度一致。
- Phase 3.3 spec 更新：sandbox-executor.md §12.2 补 DNS 族段（实证、特征、
  宁缺勿滥锚、与 listen 族的边界关系）。
