<!-- Moved from popover-pattern.md 2026-09-19 (doc-split) -->

## Code Skeleton (Copy-paste Starting Point)

```vue
<script setup lang="ts">
// MyNewPopover — hand-rolled popover following the project pattern.
// Trigger at the top of its container → opens downward. To open
// upward (bottom-of-container trigger), swap the CSS in <style>.

import { ref, onMounted, onUnmounted } from "vue";

const open = ref(false);
const root = ref<HTMLElement | null>(null);

function toggle() { open.value = !open.value; }
function close()  { open.value = false; }

function onDocumentClick(e: MouseEvent) {
  if (!open.value) return;
  const target = e.target as Node | null;
  if (root.value && target && !root.value.contains(target)) {
    open.value = false;
  }
}

function onKeydown(e: KeyboardEvent) {
  if (open.value && e.key === "Escape") {
    open.value = false;
  }
}

onMounted(() => {
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => {
  document.removeEventListener("click", onDocumentClick);
  document.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <div ref="root" class="mnp">
    <button
      type="button"
      class="mnp__trigger"
      :aria-haspopup="'menu'"
      :aria-expanded="open"
      @click="toggle"
    >
      <slot name="trigger" />
    </button>

    <div
      v-if="open"
      class="mnp__menu"
      role="menu"
    >
      <slot />
    </div>
  </div>
</template>

<style scoped>
.mnp {
  position: relative;
  display: inline-block;
}

.mnp__trigger {
  background: transparent;
  border: 0;
  cursor: pointer;
  font: inherit;
  color: inherit;
  padding: 0;
}

.mnp__menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: var(--shadow-md);
  min-width: 200px;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
}
</style>
```

Use `slot name="trigger"` for the trigger content and the
default slot for the menu body. This lets each instance
customize both without forking the popover component.

---

