# DNS 失败识别缺口——实证与研究结论（2026-09-25）

## 实证 session

jjh-mono 项目 session `ce51a3ba`（2026-09-25 04:54，edit 模式）：
用户「拉下代码，看最近的更新是什么」，模型跑
`git pull --no-rebase 2>&1; echo ...; git log ...`（复合命令），
实测输出（DB messages seq 4，逐字节）：

```
fatal: unable to access 'http://www.lj2.top:3000/jinjihu/jjh-mono.git/': Could not resolve host: www.lj2.top
```

宿主验证：`getent hosts www.lj2.top` → 116.62.239.8，HTTP 302 通——网络本身正常，
失败 100% 归因沙箱。

## 根因链（三层）

1. **检测面缺口**：`classify_block`（sandbox/mod.rs:817）网络特征只认
   listen/EPERM 形态（`operation not permitted` ci / `listen EPERM` /
   `listen tcp`+ONP / `PermissionError`+`socket`）。seccomp 拦的是
   `socket(AF_INET/AF_INET6)` 创建（seccomp.rs:3，EPERM），getaddrinfo 连
   resolver（127.0.0.53 stub，UDP，AF_INET）都建不了 socket → EPERM 被
   glibc resolver 吞掉 → 表面化为 DNS 失败文案：
   - curl/git 家族：`Could not resolve host: <host>`（git 带 `fatal: unable to access` 前缀）
   - glibc getaddrinfo 家族：`Temporary failure in name resolution`（EAI_AGAIN）
   两个家族一个都不匹配 → 无引导行、无前台升级卡、无 grant 短路、无后台 offer，
   模型只能自行诊断（本次 session 实际发生，给了正确结论但无出路）。
2. **exit-0 复合掩蔽**：shell.rs:751 闸 = `sandbox_applied && !reran_unsandboxed
   && !cancelled && exit_code != 0`。复合命令末段成功 → 整体 exit 0 →
   即使特征命中也不触发。（本 session 的复合命令即此形态；listen 族同边界，
   本任务不放开此闸。）
3. **文案面缺口**：`failure_guidance_for_kind` Network Edit 档只说「改网络
   策略或自己跑」，不知道 durable prefix grant（spec §14）这条现成通道：
   单命令 `git pull` → 弹卡「总是允许」→ `project_shell_grants` 写前缀 →
   之后同前缀免沙箱启动（DNS 通）。复合命令 grant_gate 永不命中
   （shell_trust.rs:460），文案应引导单命令。

## 与 §12 listen 缺口的关系

本缺口是 spec `.trellis/spec/backend/sandbox-executor.md` §12（listen 场景
识别缺口，2026-09-21 临时修复 + 09-22 补修）的**姊妹缺口**：listen 族
（出站 bind 方向）已收口，connect/DNS 方向（git pull/fetch、curl、wget 等
短命出网命令）漏。修复纪律完全沿用 §12.2 先例：

- 宁缺勿滥：stdout 裸 `Operation not permitted` / `Permission denied` 不认；
- 强特征两流都喂（工具链报哪条流任意；本实证即 stdout，因 `2>&1` 重定向）；
- R9 归因合取：Network 归类仅当 spawn 实装 INetBlock（`net == InetBlock`），
  服务端事实，命令输出不可伪造归因；
- 升级卡证据行 MARKERS 同步补锚（escalation.rs:280 / stdout_net_evidence_line，
  09-22 先例：不补则卡上证据行取错行——listen 修时取到 "Node.js v24.15.0" 尾行）；
- 测试锚逐字节入 live 形态。

## DNS 族字符串选择（提议）

- `could not resolve host`（大小写不敏感）——curl/git/wget 家族通用
- `temporary failure in name resolution`（大小写不敏感）——glibc getaddrinfo

不收 `Name or service not known`（EAI_NONAME）：健康宿主真 NXDOMAIN 同文案，
误归因面大；而 Block 档下 resolver 不可达必然表现为上两族，收窄无损覆盖。

误报面分析：`grep/cat` 日志自引用（如 `grep -r "Could not resolve host" .`）
会命中字符串，但命中时 exit=0 → 调用侧 `exit_code != 0` 闸已消掉该面；
cat 日志 + 命令自身失败的组合罕见，且误报代价 = 一张可拒的卡 + 一行引导，
与 Write 族 `Read-only file system` 的既有误报容忍度一致。

## 受益面（无需各自改动，classify 单源点亮）

- 前台 shell：guidance 行（shell.rs:752）+ P3c 升级闭环（escalation.rs：
  grant 短路 → 免沙箱重跑 / 无 grant → Ask 卡 → AllowAlways 写 durable grant）
- 后台 shell：P3d offer（background_escalation.rs:226 烘焙 kind）含「断网」档
- 语义：`git pull` 单命令被拦 → 卡 → 总是允许 → `git pull` 前缀 grant →
  之后免沙箱启动，「沙箱内拉代码」诉求经既有设计通道走通

## 结构性替代（本任务不做，用户已选轻量）

spec §12.3.A `readwrite_net` 第四档 / connect 端口白名单（§13 BindOnly
connect 集 {80,443}∪bind 对 3000 端口 Gitea 不覆盖）——若未来「免 grant
沙箱内直连」成为主流诉求再立项。

## 关键文件锚

- `app/src-tauri/src/sandbox/mod.rs:817` classify_block、:859 stream_smells_net_block、
  :892 failure_guidance_for_kind（Network Edit 档文案）
- `app/src-tauri/src/sandbox/seccomp.rs:78` build_inet_block_filter（socket→EPERM）
- `app/src-tauri/src/tools/shell.rs:751` exit!=0 闸 + guidance 追加点
- `app/src-tauri/src/agent/permissions/escalation.rs:280` stderr MARKERS、
  :304 stdout_net_evidence_line
- `app/src-tauri/src/agent/permissions/shell_trust.rs:460` grant_gate（复合命令永拒）
- `app/src-tauri/src/sandbox/tests_sandbox.rs:1189/1221/1296` listen 族测试锚先例
- spec：`.trellis/spec/backend/sandbox-executor.md` §10（升级闭环）§12（识别缺口）§14（grant）
