<script setup lang="ts">
// AppLogo — the small monogram shown at the far left of the custom
// title bar. Inline SVG (no external file) so it inherits `currentColor`
// from its parent — by default we use `var(--color-accent)` for the
// fill, but the parent can override it by setting `color:` on the
// wrapper.
//
// D6: added per user request "顶栏最左侧放一个 app logo (没有就随便
// 找一个或画一个 svg)". The shape is a stylised capital "E" with
// two horizontal "code bar" cuts (the middle bar is shorter, giving
// the look of an abstract terminal/code window). Size 20×20 by default
// to match the rest of the chrome.

import { withDefaults } from "vue";

withDefaults(
  defineProps<{
    size?: number | string;
  }>(),
  { size: 20 },
);
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
    :class="$attrs.class"
  >
    <!--
      Stylised "E" monogram: outer frame plus two inner cuts.
      Drawn as a single path so it stays crisp at any size and the
      accent fill works in both light and dark themes.
    -->
    <path
      d="M4 4 H20 V8 H8 V10 H17 V14 H8 V16 H20 V20 H4 Z"
      fill="currentColor"
      opacity="0.95"
    />
  </svg>
</template>
