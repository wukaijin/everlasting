<script setup lang="ts">
// PermissionGrantItem — a single row in the PermissionGrantsModal
// list. Renders one `PermissionGrantRow` (a remembered "always
// allow" grant) with:
//   1. A kind badge (整工具 / 路径 / 前缀) — color-coded so the
//      user can tell at a glance whether the grant is whole-tool,
//      a filesystem path glob, or a shell command prefix.
//   2. The tool name (mono).
//   3. The match_value (mono): the path glob (e.g. "src/*") or
//      shell prefix token (e.g. "git"); shown as "—" for the
//      `tool` kind whose match_value is NULL.
//   4. The granted_at timestamp.
//   5. A "撤销" button emitting `revoke` with the full row (the
//      modal's store revokes by the four-tuple PK).

import { computed } from "vue";
import Icon from "../Icon.vue";
import type { PermissionGrantRow } from "../../stores/permissionGrants";

const props = defineProps<{ row: PermissionGrantRow }>();
const emit = defineEmits<{ (e: "revoke", row: PermissionGrantRow): void }>();

/** Badge label + color for the match_kind. `tool` = neutral (the
 *  broadest grant), `path` = emerald (filesystem scope), `prefix` =
 *  violet (shell scope). Colors reuse the existing `--color-tool-*`
 *  family per design-tokens.md "Don't add a new `--color-*` token
 *  for a one-off use". */
const kindMeta = computed<{ label: string; colorVar: string }>(() => {
  switch (props.row.matchKind) {
    case "path":
      return { label: "路径", colorVar: "var(--color-tool-write)" };
    case "prefix":
      return { label: "前缀", colorVar: "var(--color-tool-thinking)" };
    case "tool":
    default:
      return { label: "整工具", colorVar: "var(--color-text-muted)" };
  }
});

const hasValue = computed<boolean>(() => props.row.matchValue !== null);
</script>

<template>
  <li class="grant-item">
    <span
      class="grant-item__kind"
      :style="{
        color: kindMeta.colorVar,
        borderColor: `color-mix(in srgb, ${kindMeta.colorVar} 35%, transparent)`,
      }"
    >
      {{ kindMeta.label }}
    </span>

    <div class="grant-item__body">
      <div class="grant-item__head">
        <span class="grant-item__tool">{{ row.toolName }}</span>
        <code v-if="hasValue" class="grant-item__value">{{ row.matchValue }}</code>
        <span v-else class="grant-item__value grant-item__value--null">—</span>
      </div>
      <time class="grant-item__time">{{ row.grantedAt }}</time>
    </div>

    <button
      type="button"
      class="grant-item__revoke"
      title="撤销此放行"
      @click="emit('revoke', row)"
    >
      <Icon name="trash" :size="12" />
      <span>撤销</span>
    </button>
  </li>
</template>

<style scoped>
.grant-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  transition: background var(--duration-fast) var(--ease-out);
}

.grant-item:hover {
  background: var(--color-bg-elevated);
}

.grant-item__kind {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: 999px;
  border: 1px solid;
  font-weight: var(--weight-medium);
  white-space: nowrap;
  flex-shrink: 0;
}

.grant-item__body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.grant-item__head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.grant-item__tool {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

.grant-item__value {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  word-break: break-all;
  line-height: 1.4;
}

.grant-item__value--null {
  color: var(--color-text-muted);
  font-family: var(--font-sans);
}

.grant-item__time {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.grant-item__revoke {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  padding: 4px 8px;
  background: transparent;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  font-family: inherit;
  flex-shrink: 0;
  transition: background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}

.grant-item__revoke:hover {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
  border-color: var(--color-tool-error);
  color: var(--color-tool-error);
}
</style>
