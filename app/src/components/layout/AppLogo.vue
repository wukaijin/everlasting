<script setup lang="ts">
// AppLogo — the brand mark (interlocked double loops) shown at the far
// left of the title bar (TitleBar) and the browser header
// (BrowserHeader). Inline SVG (no external file) so it ships with the
// app shell and stays crisp at any size.
//
// 2026-08-16: replaced the D6 placeholder monogram ("E" with code-bar
// cuts) with the real mark from `brand/logo-mark.svg`. Rendered at
// 16–24px we must use the favicon variant geometry — stroke 48 instead
// of 30, no center spark (unreadable below ~24px; brand README 尺寸
// 阶梯). Palette is the dark-surface one (cyan/amber); brand colors
// are fixed and do NOT follow --color-accent — the logo does not
// re-theme with the UI.

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
    viewBox="0 0 512 512"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
    :class="$attrs.class"
  >
    <g stroke-width="48" stroke-linecap="round">
      <path
        d="M 280.88 215.64 A 118 118 0 1 0 280.88 296.36"
        stroke="#22d3ee"
      />
      <path
        d="M 231.12 215.64 A 118 118 0 1 1 231.12 296.36"
        stroke="#fbbf24"
      />
    </g>
  </svg>
</template>
