# 侧栏会话项键盘可达:li 补 role/tabindex/Enter-Space

## Goal

侧栏会话项是 `<li class="session-item">` 直接挂 @click——无 tabindex、无
role、不响应键盘,键盘用户无法切换会话(WCAG 2.1.1 Keyboard)。
focus-visible 任务(08-22)全局焦点环基线已把 `[tabindex]` 纳入环覆盖,
本任务补上"能聚焦、能操作"的另一半。

## 现状(2026-08-24 实证)

- SessionList.vue 两条渲染路径(过滤平铺 L445-507、分组 L535+)的
  session-item 均为裸 li:@click 切换 / @dblclick 重命名 / @contextmenu 菜单。
- 同文件分组的 SessionGroupHeader.vue **已有现成范式**:
  `role="button" tabindex="0" :aria-expanded @keydown.enter.prevent
  @keydown.space.prevent` ——本任务照抄该范式。
- 项内删除按钮是真 `<button>`(可达);编辑态 input 自带键盘流
  (enter 提交/escape 取消)。

## Requirements

- **R1** 两条渲染路径的 session-item li 补:
  `role="button"`、`tabindex="0"`、
  `@keydown.enter="onClick(s.id)"`、`@keydown.space.prevent="onClick(s.id)"`、
  `:aria-current="active ? 'true' : undefined"`(当前会话语义)。
- **R2** 键盘重命名入口:`@keydown.shift.enter="startEditing(s.id, s.title)"`,
  注释注明与 @dblclick 对应;ctx 菜单的键盘打开(Shift+F10)不在本任务,
  记 follow-up。
- **R3** 焦点呈现零新增代码——全局 :focus-visible 基线的 `[tabindex]`
  选择器自动给环;验证时确认 computed boxShadow 命中即可。
- **R4** 行为不变:鼠标路径(click/dblclick/contextmenu)原样;
  space.prevent 防滚动副作用仅作用在项上。

## Acceptance Criteria

- [ ] 运行时键盘探针:Tab 落到 session-item 时 document.activeElement
      匹配 `:focus-visible` 且 boxShadow 为 --shadow-ring;按 Enter 后
      store 切到该会话(界面 active 态迁移);
- [ ] Shift+Enter 进入编辑态(input 出现并获得焦点),Escape/Enter 可退出;
- [ ] pnpm test 全绿 + pnpm build 零错;
- [ ] SessionGroupHeader 范式未被破坏(分组头仍可 Enter 折叠)。

## Notes

- role="button" 内嵌真 button(delete)是无障碍嵌套瑕疵,现状即如此
  (视觉上 delete 常驻);不在此展开重构,记 follow-up。
- 键盘可达后,ctx 菜单(重命名/设色/删除)对键盘用户仍只有 Shift+Enter
  重命名一条路;菜单键盘打开记 follow-up。
