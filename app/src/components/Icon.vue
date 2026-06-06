<script setup lang="ts">
// Icon — thin wrapper around @heroicons/vue. Centralises the icon
// registry so individual components don't have to import every icon
// they might use. Icons come from the 24/outline set, which is the
// standard "stroke" style suitable for UI work in a dark theme.
//
// To add a new icon, import the matching component from
// "@heroicons/vue/24/outline" and add it to the `map` object below.

import { computed } from "vue";
import {
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
} from "@heroicons/vue/24/outline";

const props = withDefaults(
  defineProps<{
    /** Icon name (key of the registry below). */
    name: string;
    /** Width and height in px. Defaults to 16. */
    size?: number | string;
    /** Additional class names applied to the <svg>.
     *  Named `iconClass` to avoid colliding with Vue 3's
     *  automatic root-element `class` attribute merging. */
    iconClass?: string;
  }>(),
  { size: 16 },
);

const map = {
  "arrow-up": ArrowUpIcon,
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
} as const;

const Component = computed(() => {
  const c = map[props.name as keyof typeof map];
  return c ?? null;
});
</script>

<template>
  <component
    :is="Component"
    v-if="Component"
    :size="size"
    :class="['icon', iconClass]"
    aria-hidden="true"
  />
</template>

<style scoped>
.icon {
  display: inline-block;
  vertical-align: middle;
  flex-shrink: 0;
}
</style>
