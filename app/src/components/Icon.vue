<script setup lang="ts">
// Icon — thin wrapper around @heroicons/vue + @lucide/vue.
// Centralises the icon registry so individual components don't have
// to import every icon they might use. Heroicons come from the 24/
// outline set; lucide is mixed in for icons heroicons doesn't ship
// (e.g. `brain` for the Memory entry, 2026-06-11).
//
// To add a new icon, import the matching component from
// "@heroicons/vue/24/outline" (or "@lucide/vue" for icons the
// heroicons set lacks) and add it to the `map` object below.
//
// NOTE: @heroicons/vue@2.x components are render functions that emit
// an <svg> with NO width/height attributes — they must be sized via
// CSS. @lucide/vue components ship default `width`/`height` props
// (default 24); they are also CSS-sizable because the wrapping
// `:deep(svg)` rule below forces width/height to 100%. The two
// libraries coexist without further glue.

import { computed } from "vue";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  CheckIcon,
  XMarkIcon,
  DocumentTextIcon,
  PencilSquareIcon,
  CommandLineIcon,
  WrenchIcon,
  ExclamationTriangleIcon,
  ArchiveBoxIcon,
  EllipsisHorizontalIcon,
  ArrowPathIcon,
  ChatBubbleLeftRightIcon,
  PlusIcon,
  PaperAirplaneIcon,
  LockClosedIcon,
  Squares2X2Icon,
  ArrowsPointingOutIcon,
  FolderIcon,
  MinusIcon,
  CogIcon,
  Cog6ToothIcon,
  EyeIcon,
  EyeSlashIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  ChevronRightIcon,
  TrashIcon,
  KeyIcon,
  SignalIcon,
  GlobeAltIcon,
  AdjustmentsHorizontalIcon,
  ServerIcon,
  CircleStackIcon,
  BoltIcon,
  ClockIcon,
  MagnifyingGlassIcon,
  UserGroupIcon,
  Bars3Icon,
} from "@heroicons/vue/24/outline";
import { Brain, ShieldX, ShieldCheck, Terminal, Copy, Info, CircleDot, Check, ClipboardList, Circle, LoaderCircle, GitMerge, FileCheck, ChartLine, Repeat, ListTree, Shrink, CornerUpRight } from "@lucide/vue";

const props = withDefaults(
  defineProps<{
    /** Icon name (key of the registry below). */
    name: string;
    /** Width and height in px. Defaults to 16. */
    size?: number | string;
    /** Additional class names applied to the wrapper <span>.
     *  Named `iconClass` to avoid colliding with Vue 3's
     *  automatic root-element `class` attribute merging. */
    iconClass?: string;
  }>(),
  { size: 16 },
);

const map = {
  "arrow-down": ArrowDownIcon,
  "arrow-up": ArrowUpIcon,
  "bars-3": Bars3Icon,
  "check": CheckIcon,
  "x": XMarkIcon,
  "document": DocumentTextIcon,
  "pencil": PencilSquareIcon,
  "command-line": CommandLineIcon,
  "wrench": WrenchIcon,
  "warn": ExclamationTriangleIcon,
  "archive": ArchiveBoxIcon,
  "ellipsis": EllipsisHorizontalIcon,
  "refresh": ArrowPathIcon,
  "thinking": ChatBubbleLeftRightIcon,
  "plus": PlusIcon,
  "send": PaperAirplaneIcon,
  "lock": LockClosedIcon,
  "maximize": Squares2X2Icon,
  "restore": ArrowsPointingOutIcon,
  "folder": FolderIcon,
  "minus": MinusIcon,
  "cog": CogIcon,
  "cog-6-tooth": Cog6ToothIcon,
  "eye": EyeIcon,
  "eye-slash": EyeSlashIcon,
  "chevron-down": ChevronDownIcon,
  "chevron-up": ChevronUpIcon,
  "chevron-right": ChevronRightIcon,
  "trash": TrashIcon,
  "key": KeyIcon,
  "signal": SignalIcon,
  "globe": GlobeAltIcon,
  "adjustments": AdjustmentsHorizontalIcon,
  "server": ServerIcon,
  "circle-stack": CircleStackIcon,
  "bolt": BoltIcon,
  "clock": ClockIcon,
  // 2026-06-27 sidebar 搜索入口: MagnifyingGlassIcon 触发搜索 input 行
  "magnifying-glass": MagnifyingGlassIcon,
  // 08-04 group-chat UI: 群聊入口按钮 (Sidebar "新建群聊") + 群聊
  // 头部编辑 chip (ChatPanel) 共用。UserGroupIcon 比 UsersIcon 语义
  // 更贴"群聊"。此前两处调用 name="users" 但未注册, Icon 渲染成空 span。
  "users": UserGroupIcon,
  "brain": Brain,
  // PR3 (A2 + B7): PermissionModal visuals — lucide icons
  // for the shield/terminal/copy/info family. Heroicons
  // doesn't ship a `shield-x` variant; we use lucide's.
  "shield-x": ShieldX,
  "shield-check": ShieldCheck,
  "terminal": Terminal,
  "copy": Copy,
  "info": Info,
  "circle-dot": CircleDot,
  "check-mini": Check,
  // 3 档 Mode UI (2026-06-13): "pencil" reused (heroicons
  // PencilSquareIcon already in registry above); only need to
  // add the lucide clipboard-list for Plan.
  "clipboard-list": ClipboardList,
  // B12 ChecklistCard (2026-06-19): all three status icons are
  // lucide (project preference for the cleaner line weight on
  // the spinner). `circle` is the empty outline for pending;
  // `loader` is the classic spinner circle for in_progress (CSS
  // `checklist-spin` rotates it); `check-mini` is the check
  // mark for done.
  "circle": Circle,
  "loader": LoaderCircle,
  // L3b PR4 (2026-06-27): WorkerMergeControls merge button —
  // heroicons doesn't ship a git-merge variant; lucide's line
  // weight matches the existing shield-x / clipboard-list
  // family already pulled in.
  "git-merge": GitMerge,
  // B9+ D4 (2026-07-13): AuditLogItem renders a "file-check"
  // icon for `ui_diff_applied` rows. lucide ships the variant
  // directly; reuses the same lucide family as `shield-x` /
  // `git-merge` above.
  "file-check": FileCheck,
  // E2 (harness trace pipeline, 2026-07-14): trace timeline
  // icon family. lucide ships these directly; reuses the same
  // line weight as the existing `shield-x` / `git-merge` /
  // `file-check` group so the chip-family reads consistently.
  // `chart-line` is the drawer toggle (TracePanel icon in
  // ChatPanel header); `repeat` is the loop-detection
  // sub-card icon; `list-tree` is the workflow breadcrumb
  // sub-card icon; `shrink` is the C3 compaction sub-card
  // icon (mirrors the "shrinking context" semantics).
  "chart": ChartLine,
  "repeat": Repeat,
  "list-tree": ListTree,
  "shrink": Shrink,
  // handoff (08-18-handoff-mechanism): 接力行卡片图标 ——
  // "corner-up-right" 表"续跑/接力"语义,与 shrink(收窗)对仗。
  "corner-up-right": CornerUpRight,
} as const;

const Component = computed(() => {
  const c = map[props.name as keyof typeof map];
  return c ?? null;
});

const sizeStyle = computed(() => {
  const s = typeof props.size === "number" ? `${props.size}px` : String(props.size);
  return {
    width: s,
    height: s,
    "flex-shrink": 0,
    display: "inline-flex",
  };
});
</script>

<template>
  <span
    v-if="Component"
    :class="['icon', iconClass]"
    :style="sizeStyle"
    aria-hidden="true"
  >
    <component :is="Component" />
  </span>
</template>

<style scoped>
.icon {
  vertical-align: middle;
}
.icon :deep(svg) {
  width: 100%;
  height: 100%;
  display: block;
}
</style>
