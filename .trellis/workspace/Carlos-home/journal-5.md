# Journal - Carlos-home (Part 5)

> Continuation from `journal-4.md` (archived at ~2000 lines)
> Started: 2026-09-20

---



## Session 153: checkpoint 轮间徽标 + diff 弹窗四修(timeline footer 缺口 / z-index 陷阱 / 净零轮闸)

**Date**: 2026-09-20
**Task**: checkpoint 轮间徽标 + diff 弹窗四修(timeline footer 缺口 / z-index 陷阱 / 净零轮闸)
**Branch**: `main`

### Summary

jjh-mono 实测回归驱动的 checkpoint UI 收敛:(1) MessageItemFooter 新增 checkpoint 徽标(store 增 filesChangedAt,与 hasTurnDiff 同闸),点击直开「本轮 diff」弹窗;(2) 根因修复 timeline 行 footer 双挂载点全灭——纯 thinking+tool_use 轮 msg__tools 被 !useTimeline 压掉、外层挂载点被 tools/no-bubble 压掉,耗时 chip 与徽标一起消失,外层 v-if 补 || useTimeline;(3) hasTurnDiff 加 files_changed>0 闸,净零轮(gitignore 工作区编辑)不给必空入口,revert 入口不受影响;(4) DiffModal/RevertConfirmModal Teleport 到 body + DiffModal 升 modal 家族 z 档——MessageList 虚拟行 inline transform 祖先把 fixed 弹层困进行级 stacking context,zindex ladder 注释与 shadow-zindex-tokens.md spec 同步并新增 Teleport 规则;(5) DiffModal 重设计:家族遮罩 blur、头部 file-diff 图标+文件数/±行 meta、body min-height 220px 防空态塌缩、空载荷专属空态。验证:vitest 1991 全过、vue-tsc build 过、Playwright 真实 Chromium 全会话逐行扫描——徽标精确落在 15/17/19/21/23/103/105 七行,与后端 files_changed>0 一一对应;Teleport 断言(backdrop 挂 body、z=2000、elementFromPoint 命中弹窗)全过。测试侧:RevertConfirmModal 测试查询改走 document.body(Teleport 内容对 wrapper.find 不可见),store 补 filesChangedAt 用例,footer 补徽标四臂。

### Git Commits

| Hash | Message |
|------|---------|
| `eb17d071` | (see git log) |

### Status

[OK] **Completed**


## Session 154: 错误链路收口:TransportError category 恢复 + 全局兜底分级

**Date**: 2026-09-21
**Task**: 错误链路收口:TransportError category 恢复 + 全局兜底分级
**Branch**: `main`

### Summary

根治「服务端错误 ResizeObserver loop…」误报:审计出全局兜底与传输层两 P1 断点(裸 string 硬标 Server、Error 实例被静默丢弃;TransportError 丢 category/retryable)。经群聊评审(7 条裁决,抓到 status=0 兜 Server 分档缺陷 + requestId camelCase wire 前科)后实现:TransportError 携带四字段成为 AppCommandError 形状(body→canonical 逆映射→分档兜底)、useErrorBus.handle 四级分类(形状识别→良性噪音 debug→string warn→Error error)、main.ts 噪音过滤 + Vue errorHandler。2013 测试全绿,vue-tsc 零错。spec 补齐 error-handling.md 四层模型(此前是模板)。遗留:AC1 GUI 冒烟与 401 叠加验收待人工;P2(双 toast 合并、静默面横扫、诊断 UI)待开任务。

### Git Commits

| Hash | Message |
|------|---------|
| `aefd929b` | (see git log) |
| `1c2d80e0` | (see git log) |

### Status

[OK] **Completed**


## Session 155: durable prefix grant live E2E 全绿 + classify_block stderr 识别缺陷修复

**Date**: 2026-09-22
**Task**: durable prefix grant live E2E 全绿 + classify_block stderr 识别缺陷修复
**Branch**: `main`

### Summary

补上 09-21-durable-prefix-grant 最后一条 AC:本机 WSL2 5.15(landlock 有/landlock_net 无/seccomp 有,net=block)三段 live E2E 全绿——Phase1 沙箱拦裸 node dev server→升级卡(信任面文案+grantPattern 回显)→allow_always→durable 行+审计→免沙箱重跑成功;Phase2 新 session 同前缀零弹卡直接免沙箱启动(tool_allowed durable prefix grant hit),不同前缀仍进沙箱;Phase3 管理面 list/revoke+grant_revoked 审计。驱动面:daemon HTTP API+SSE 拦截 permission:ask(脚本 /tmp/durable-grant-e2e/)。E2E 首轮暴露既有缺陷:裸 node 崩溃把 listen EPERM 打在 stderr 且 libuv errno 小写,classify_block 只认 stderr 大写 O 字面量+强特征只喂 stdout→升级链整条哑火;修复=errno 大小写不敏感+stream_smells_net_block 双流+证据行 listen 锚(live 形态逐字节测试锚,全量 --lib 2592 绿,spec §12.2 补修+§14.5 更新),daemon 已重建重启验证。任务随 AC 12/12 全勾收档(archive 53848065)。遗留备注:spec sandbox-executor.md 37KB 已超 context 注入 32KB 截断阈值(validate 警告),后续可拆分。

### Git Commits

| Hash | Message |
|------|---------|
| `f2f7776b` | (see git log) |
| `de08f499` | (see git log) |

### Status

[OK] **Completed**


## Session 156: 沙箱 DNS 失败识别缺口收口——classify_block 补 connect/resolve 特征族

**Date**: 2026-09-25
**Task**: 沙箱 DNS 失败识别缺口收口——classify_block 补 connect/resolve 特征族
**Branch**: `main`

### Summary

从 DB 最新会话排查「拉不到远程」：jjh-mono session ce51a3ba 的 git pull 被 seccomp 断网拦（socket AF_INET EPERM→DNS 失败文案），classify_block 只认 listen 形态致升级卡/grant/引导全链哑火。轻量修立项 09-25-sandbox-dns-block-detect：dns_smells_net_block 两族双流 ci（不收 Name or service not known）、R9 合取结构保证、卡证据行 MARKERS 补锚、Network Edit 档文案补 prefix grant 出路（点名 git pull 单命令）；测试锚含逐字节实证形态，spec §12.2 补 DNS 族段。全量 cargo test -p everlasting --lib 2596 绿。效果：git pull 被拦→卡「始终允许」→前缀 grant→之后免沙箱直连。

### Git Commits

| Hash | Message |
|------|---------|
| `b2ed9969` | (see git log) |

### Status

[OK] **Completed**


## Session 157: N7 DiffView 增强:行内高亮 + side-by-side 全链交付

**Date**: 2026-09-27
**Task**: N7 DiffView 增强:行内高亮 + side-by-side 全链交付
**Branch**: `main`

### Summary

BACKLOG 附录 B N7 立项(推荐采纳后用户 OK)。brainstorm 三问裁定(同批交付/默认 unified+localStorage 记忆/EditFileCard 升级不重构)+ Q4 色路径 Q5 bench 自裁;勘察修正共识表「按文件折叠」已存在,实缺口=行内 word-diff+双栏。群聊评审(review preset,session c5460c4b,112.7 万 token,14 结论全 verified)回填:重分布改 \n 切分推进+不变量钉 util 断言、原子 null 三路径(超长4000/抛错/校验败)+view 级 run 帽200、split 必配 align-items:start、CRLF 消费点归一(util 纯净)、工具行三缺席条件、matchMedia 选树+jsdom stub+unmount 断言、localStorage 三纪律(key everlasting:diffview.mode)、e2e 空 fixture 恒真陷阱(负控先红后绿)、EditFileCard 截断点钉测、AC1 只做 DOM 断言。评审实锤「全局单键穿透视口降级」(桌面宽视口 inline ~250px 窄容器不受 768px 视口降级保护)→ allowSplit prop(默认 true)+ ToolCallCard 一行 opt-out,PRD R7/AC6 修正案任务侧采纳(依据=用户 Q2 前提,一行可回退)。实施 trellis-implement 子代理交付:intraLineDiff.ts util+13 测试、DiffView 行内/split/工具行/持久化、EditFileCard 行内+口径统一、e2e 2 用例。trellis-check 全 AC ✅(修一处 e2e 名实不符:空载荷≠raw-only)。视觉抽查:VLM 判「行内高亮看不见」被确定性数值复核推翻(computed 0.28 vs 0.12、Δ绿≈16% GitHub 同量级)——VLM 对 13px 小色块判定不可信实证入 spec。终态 vitest 2095 绿(基线2056,+39)/vue-tsc 零错/e2e 30/30 真实 Chromium。spec 备案:generative-ui.md N7 渲染契约节、responsive-mobile.md matchMedia 选树例外、ROADMAP/BACKLOG 记账。

### Git Commits

| Hash | Message |
|------|---------|
| `073c7f5a` | (see git log) |

### Status

[OK] **Completed**
