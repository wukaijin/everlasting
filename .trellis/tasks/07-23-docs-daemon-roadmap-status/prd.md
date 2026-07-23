# PRD: docs sync B — 路线图/状态/索引文档

> 父任务:`../07-23-docs-sync-daemon-split/`
> 审计依据:`../07-23-docs-sync-daemon-split/research/audit-daemon-docs-drift.md` §B1-B5
> 跨任务一致性约定:见父 prd.md

## 目标

让项目的路线图、顶层 README、术语表、backlog、文档索引反映 daemon 化已落地,消除「daemon 化是飞书 B10 触发的未来计划」叙事。

## 范围

### B1. `docs/ROADMAP.md`
- §1.2 已实施表(行 37-92):新增 daemon 化 epic 条目,覆盖 transport 抽象 + sidecar spawn + httpTransport 默认 + ServeDir + 浏览器模式 + E2E,带 commit 引用(`0dbc747`→`3307d93`)
- §2 第四档 B10(行 140):「触发 daemon 化,重大架构变更」→ 触发条件已满足,daemon 化已作为独立基础设施完成,B10 可基于既有 daemon/transport 推进
- §2 四档分类:确认 daemon 化归位(已在 §1.2 已实施,不应再悬在计划盲区)

### B2. `README.md`(顶层)
- 状态行(行 21):07-17 → 07-23,「25 项」含 daemon 化
- 能力矩阵(行 54-88):新增「运行形态」小节(Tauri GUI / 浏览器模式 / daemon sidecar / transport 抽象)
- 约束(行 110):「不做 Web 版」澄清 —— 本地浏览器模式(localhost 访问本机 daemon)≠ 托管云端 Web 版

### B3. `docs/CONTEXT.md`(glossary)
- 新增术语定义:`everlasting-daemon`(bin) / `httpTransport` / `tauriTransport` / transport 抽象层 / `sidecar` spawn / `ServeDir` / 浏览器模式 / `GuiMode`(Thin/Full)
- 顺手修内部矛盾:行 133「Checklist 规划中」vs 行 53「已落地」
- 顺手修数值:行 115 AuditKind「10 类」→ 20+

### B4. `docs/BACKLOG.md`
- §4 跨设备(行 91-132):精确区分「本地 daemon 化(已做,指向 ROADMAP §1.2)」vs「VPS 跨设备(未做)」。行 101「暂定方向/留接口」、行 106「前期已留的接口」需改为「本地部分已落地」
- 行 107 Channel Adapter 协议:已升级为实际 httpTransport,改措辞
- §3.2(行 57):「集中到 agent daemon」未来式 → 现在时

### B5. `docs/README.md`(文档索引)
- 文档结构表(行 12-27):补 daemon/transport 专题文档条目(`REMOTE-ACCESS-ROADMAP.md` / `REMOTE-ACCESS-RESEARCH.md` / `MANUAL-TEST-P2.md` / `_reviews/REVIEW-remote-access-*`)
- 行 18 CONTEXT.md 条目描述「A4 Token 术语」更新为实际范围

## 验收标准

- [ ] ROADMAP §1.2 有 daemon 化 epic 条目 + commit 引用
- [ ] ROADMAP §2 B10「触发 daemon 化」措辞修正(触发条件已满足)
- [ ] README 状态行 ≥ 07-23,能力矩阵含运行形态小节
- [ ] README 行 110「不做 Web 版」有浏览器模式澄清
- [ ] CONTEXT.md 新增 7+ daemon/transport 术语定义
- [ ] CONTEXT.md 内部矛盾(行 53 vs 133)修正,AuditKind 数值更新
- [ ] BACKLOG §4 区分本地 daemon(已做) vs VPS 跨设备(未做)
- [ ] docs/README.md 索引含 remote-access 专题文档条目
- [ ] 全组 grep「飞书.*触发 daemon\|触发 daemon 化.*飞书」清零(B10 条目保留飞书本身,只改触发关系)

## 风险

- ROADMAP §1.2 是 living document 的核心表,新增条目格式(列、日期、commit)要对齐既有行风格。
- BACKLOG §4 精确区分两子范围是难点,避免把「VPS 跨设备未做」误标成已完成。
