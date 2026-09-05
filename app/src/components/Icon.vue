<script setup lang="ts">
// Icon — thin wrapper around @lucide/vue.
// Centralises the icon registry so individual components don't have
// to import every icon they might use. All icons come from lucide's
// 24px outline set. (2026-09-02: migrated off @heroicons/vue — some
// registry keys still carry heroicons-era names, e.g. `bars-3` /
// `cog-6-tooth` / `magnifying-glass`; kept stable so callers don't
// churn.)
//
// To add a new icon, import the matching component from "@lucide/vue"
// and add it to the `map` object below.
//
// NOTE: @lucide/vue components ship default `width`/`height` props
// (default 24); they are CSS-sizable because the wrapping `:deep(svg)`
// rule below forces width/height to 100%.

import { computed } from "vue";
import {
  Activity,
  Archive,
  ArrowDown,
  ArrowUp,
  Brain,
  Calendar,
  ChartLine,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Circle,
  CircleDot,
  ClipboardList,
  Clock,
  Cog,
  Copy,
  CornerUpRight,
  Database,
  Ellipsis,
  Expand,
  Eye,
  EyeOff,
  FileCheck,
  FileText,
  Folder,
  GitMerge,
  Globe,
  Info,
  KeyRound,
  LayoutGrid,
  ListTree,
  LoaderCircle,
  Lock,
  Menu,
  MessagesSquare,
  Minus,
  Palette,
  Plus,
  RefreshCw,
  Repeat,
  Search,
  Send,
  Server,
  Settings,
  ShieldCheck,
  ShieldX,
  Shrink,
  Signal,
  SlidersHorizontal,
  Square,
  SquarePen,
  SquareTerminal,
  Terminal,
  Trash2,
  TriangleAlert,
  Users,
  Wrench,
  X,
  Zap,
} from "@lucide/vue";

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
  "arrow-down": ArrowDown,
  "arrow-up": ArrowUp,
  // bars-3 / cog-6-tooth / magnifying-glass 等 key 是 heroicons 时代的
  // 命名遗产,lucide 对应件见行尾;key 不改,调用方零迁移。
  "bars-3": Menu,
  "check": Check,
  "x": X,
  "document": FileText,
  "pencil": SquarePen,
  "command-line": SquareTerminal,
  "wrench": Wrench,
  "warn": TriangleAlert,
  "archive": Archive,
  "ellipsis": Ellipsis,
  "refresh": RefreshCw,
  "thinking": MessagesSquare,
  "plus": Plus,
  "send": Send,
  "lock": Lock,
  // TitleBar 窗口控件:maximize 是 2×2 格(视觉沿用 heroicons
  // squares-2x2),restore 是向外箭头。
  "maximize": LayoutGrid,
  "restore": Expand,
  "folder": Folder,
  "minus": Minus,
  "cog": Cog,
  "cog-6-tooth": Settings,
  "eye": Eye,
  "eye-slash": EyeOff,
  "chevron-down": ChevronDown,
  "chevron-up": ChevronUp,
  "chevron-right": ChevronRight,
  "chevron-left": ChevronLeft,
  "trash": Trash2,
  "key": KeyRound,
  "signal": Signal,
  "globe": Globe,
  "adjustments": SlidersHorizontal,
  "server": Server,
  "circle-stack": Database,
  "bolt": Zap,
  // 界面主题切换(经典/激进,Sidebar footer 快速入口)。
  "palette": Palette,
  "clock": Clock,
  // 08-29 定时任务日期控件(AppDatePicker)。
  "calendar": Calendar,
  // 2026-06-27 sidebar 搜索入口: 触发搜索 input 行。
  "magnifying-glass": Search,
  // 08-04 group-chat UI: 群聊入口按钮 (Sidebar "新建群聊") + 群聊
  // 头部编辑 chip (ChatPanel) 共用。Users 比 User 语义更贴"群聊"。
  // 此前两处调用 name="users" 但未注册, Icon 渲染成空 span。
  "users": Users,
  "brain": Brain,
  // PR3 (A2 + B7): PermissionModal visuals — shield/terminal/copy/info
  // icon family.
  "shield-x": ShieldX,
  "shield-check": ShieldCheck,
  "terminal": Terminal,
  "copy": Copy,
  "info": Info,
  "circle-dot": CircleDot,
  "check-mini": Check,
  // 3 档 Mode UI (2026-06-13): "pencil" reused (registry above);
  // clipboard-list is the Plan-mode icon.
  "clipboard-list": ClipboardList,
  // B12 ChecklistCard (2026-06-19): all three status icons.
  // `circle` is the empty outline for pending; `loader` is the
  // classic spinner circle for in_progress (CSS `app-spin` rotates
  // it, via ChecklistCard's :deep(svg) rule); `check-mini` is the
  // check mark for done.
  "circle": Circle,
  "loader": LoaderCircle,
  // L3b PR4 (2026-06-27): WorkerMergeControls merge button — line
  // weight matches the shield-x / clipboard-list family.
  "git-merge": GitMerge,
  // B9+ D4 (2026-07-13): AuditLogItem renders a "file-check" icon
  // for `ui_diff_applied` rows.
  "file-check": FileCheck,
  // E2 (harness trace pipeline, 2026-07-14): trace timeline
  // icon family. `chart-line` is the drawer toggle (TracePanel icon
  // in ChatPanel header); `repeat` is the loop-detection
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
  // 2026-09-02 (task 09-02-chat-task-panel): ActivityPanel 状态图标
  // 补充 —— `square` 是 killed / timed_out 后台命令的中性终态图标;
  // `activity` 是面板浮球/标题的「运行状态」图标。
  "square": Square,
  "activity": Activity,
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
