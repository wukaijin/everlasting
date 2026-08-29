# PRD: 定时任务表单换用 reka-ui 日期/时间控件

## 背景

`ScheduledTasksTab.vue`(设置 → 定时任务)的触发计划/结束条件目前用原生
`input[type=datetime-local]` / `input[type=time]` / `input[type=date]`:

- Tauri webkit2gtk 的原生日期控件样式不可控(浅色、与暗色主题不一致),
  与本表单其它控件(reka Select)视觉割裂 —— 该表单 2026-08-28 已因同样
  理由把原生 `<select>` 换成 reka Select(组件头注释)。
- 数字滚轮/文本输入的交互方式不便,用户要求日历/时间控件。

## 方案

用**项目已有依赖 reka-ui 2.9.9** 的日期原语(零新组件库):

- `DatePickerRoot/Trigger/Portal/Content/Calendar/...` — 日历弹层
- `TimeFieldRoot/TimeFieldInput` — 分段式时间输入(键入数字、↑↓ 步进)
- 新增直依赖 `@internationalized/date`(reka 的 transitive dep,类型 +
  `CalendarDate` / `Time` 构造器;pnpm 隔离 node_modules 需显式声明)

新增两个通用包装组件(`app/src/components/common/`),对外暴露**字符串
v-model** 契约,把 DateValue/TimeValue 转换收敛在组件内部:

1. `AppDatePicker.vue` — 触发按钮(显示 `yyyy-MM-dd` / placeholder)+
   日历弹层;props: `modelValue: string`("yyyy-MM-dd" | "")、
   `placeholder`、`minValue`、`disabled`;emit `update:modelValue`。
2. `AppTimeField.vue` — 分段时间输入;props: `modelValue: string`
   ("HH:mm" | "")、`disabled`;emit `update:modelValue`。固定 24 小时制
   (`hour-cycle="h23"`)。

## 范围(ScheduledTasksTab 内的替换)

| 现控件 | 替换为 |
|---|---|
| 单次档 `datetime-local` | `AppDatePicker`(日期)+ `AppTimeField`(时刻)|
| daily/weekdays/weekly/monthly 的 `input[type=time]` | `AppTimeField` |
| 结束日期 `input[type=date]` | `AppDatePicker` |

表单模型:`form.onceAt` 拆为 `onceDate` + `onceTime` 两个字符串字段;
`form.at` / `form.endDate` 不变。校验逻辑(过去时刻拒绝、结束日期不早于
今天)保持原语义,另以日历 `minValue=今天` 前置拦截。

## Out of scope

- 数字输入(hourlyMinute / monthlyDay / intervalCount / maxRuns)换
  reka NumberField —— 痛点次级,留待后续任务。
- reka-ui 升级、手写 popover 改造。
- 后端任何改动(纯前端)。

## 验收标准

1. 表单不再渲染 `input[type=date|time|datetime-local]`。
2. 单次档 = 日历弹层选日期 + 分段输入选时刻;固定时间档 4 个档位的
   时刻走分段输入;结束日期走日历弹层。
3. 日历弹层禁选今天之前的日期;表单校验错误文案与现状一致。
4. 样式全走 design token(不硬编码 hex/px 灰阶),暗色主题下与
   Select 弹层观感一致;弹层 z-index 高于 settings modal
   (`--z-over-modal`);portal 子元素样式按 reka-ui-usage.md 用 `:deep()`。
5. 移动端 320px 无横向溢出(日历弹层宽度可控)。
6. `cd app && pnpm test` 全绿:tab 测试(新控件以 stub 注入,继续测
   表单接线/校验/提交 payload)+ 两个包装组件的单元测试(字符串 ⇄
   DateValue/TimeValue 转换、分段键入)。
7. `pnpm build`(含 vue-tsc)通过。
8. 浏览器截图验证新控件观感(daemon serve dist + 无头浏览器),交用户
   目检。

## 约束

- **用户明确要求:本轮不 commit。** 任务停在 in_progress,截图确认后
  再走提交流程。
- reka-ui 钉死 2.9.9(见 spec/frontend/reka-ui-usage.md),使用的原语
  已在该版本 dist 中确认存在。
