# 群聊禁用模型仍可选——preset 预填与全局 pinning 泄漏修复

> 2026-09-09 用户报告:「模型/提供商禁用之后,群聊里还是能选择」。排查确认
> 09-07(provider-model-disable)的六入口过滤被 09-08(gce-m4c 弹窗重设计)
> 新增的 preset 路径绕过,且「启用 ∪ 已选值」并集的实现粒度有跨行泄漏。
> 本文即决策记录,无独立 design.md(改动面收敛)。

## Root Cause(两条独立泄漏路径)

### A. preset 预填走全目录解析(09-08 新增,晚于 09-07 禁用改造)

- `GroupChatConfigModal.applyPreset` 与 `ScheduledTasksTab` 的
  `onPickGcPreset` / `gcModeratorModelId` / `expandGcConfig` 都用
  `resolveModelRef(models.models, …)` 解析 preset 模型引用 —— 全目录,
  不滤禁用。禁用的模型被静默预填进 create 阵容/主持人,`presetWarnings`
  只报「解析失败」不报「已禁用」,提交放行 → 新群聊/定时审议可整场建在
  禁用模型上。preset 引用固定模型名(glm-5.3 / MiniMax-M3 / …),禁掉
  对应 provider 即触发,与用户报告场景吻合。
- 09-07 PRD R2 语义:「禁用 = 选用层开关,被禁模型从选模型入口消失」。
  preset 预填是新「选用」入口,理应同语义。

### B. 「启用 ∪ 已选值」并集做成全局 Set(跨行/跨字段泄漏)

四处同构实现(GroupChatConfigModal / SubagentsTab / ProjectSubagentsTab /
ScheduledTasksTab)都把**所有行**的当前值收进一个 pinned Set:
任一行已指向禁用模型 X → **每一行**的下拉都提供 X → 其他行可把 X
「新选」进来。与代码注释自称的「禁用模型不再可被改选」矛盾。
09-07 PRD 的原意是「编辑态**回显的旧值**仍可见可切走」——回显是行内
概念,不是全局概念。

## Fix 语义(对齐 09-07 PRD R2,不新增后端拦截)

1. **preset 展开(create 流)只解析启用目录**:`resolveModelRef` 传
   `enabledModels`;禁用模型表现同「不在目录」—— 留空 + 警示,绝不静默
   预填。警示文案区分两态:「已被禁用,请先在「模型」页启用」vs
   「不在模型目录中,请先在「模型」页添加」(禁用态查全目录反诊)。
2. **回显 pinning 收敛到行内/字段内**:row i 下拉 = 启用 ∪ {row i 当前值};
   主持人下拉 = 启用 ∪ {moderatorId};subagent 行 = 启用 ∪ {本行
   resolvedModelId};定时表单 session 模型与 gc 主持人各自独立成表
   (原来共用一个 list,互相 pin)。别行的禁用值不再出现在本行选项里。
3. **定时提交二次拦截**:`expandGcConfig` 对解析结果追加有效禁用校验
   (覆盖「表单开着时模型被禁用」的竞态),错误文案同 1。
4. **编辑态旧阵容保留**:edit 模式 roster 已指向禁用模型的行,该行下拉
   仍显示它(可见可切走),提交放行 —— 「已在用的会话不受影响」边界不变。

## Out of Scope

- `parseForcedDispatchPrefix`(`@模型:` 手动前缀)仍走全目录:显式键入
  是 power-user 逃生口,PRD R2「分发 catalog 照常收录」语义下故意不过滤。
- 后端不加 create 拦截:禁用本就是选用层开关,非停用。

## Acceptance Criteria

- [x] 建群弹窗 create:目录含禁用模型时,行下拉/主持人下拉不出现禁用模型
- [x] 建群弹窗 preset:参与者/主持人引用禁用模型 → 留空 + 「已被禁用」
      警示 + 提交禁用;启用后恢复
- [x] 建群弹窗 edit:roster 行指向禁用模型 → 本行下拉可见可切走,提交
      放行;其他行下拉不出现该模型
- [x] 定时表单:gc 主持人下拉/展开校验同语义;session 模型下拉与 gc
      主持人下拉互不 pin
- [x] SubagentsTab / ProjectSubagentsTab:跨行泄漏修复(行内回显保留)
- [x] `cd app && pnpm test` 全绿;`vue-tsc --noEmit` 干净
