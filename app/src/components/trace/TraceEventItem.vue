<script setup lang="ts">
// TraceEventItem — thin wrapper around the existing
// `<AuditLogItem>` component. The trace viewer's per-turn
// tool-call sub-list reuses the C4 audit modal's renderer
// verbatim so visual / icon / color vocabulary stays
// consistent between the audit log and the trace timeline.
//
// Failure highlight: the trace viewer's design-tokens spec
// says `tool_executed.exit_code != 0` adds the red "critical"
// left border. AuditLogItem already drives the
// `audit-item--critical` class via `parsed.value.payload.
// critical === true` — but the tool_executed path doesn't
// carry a `critical` field. We patch the row here: the
// `tool_executed` payload's `exit_code != 0` (and `!= null`)
// flips a `_traceCritical` flag we thread onto the wrapper,
// and the wrapper itself adds the class. This keeps the
// shared `AuditLogItem` contract intact (no schema changes)
// while satisfying the trace viewer's stricter "any
// non-success exit" coloring.

import { computed } from "vue";
import AuditLogItem from "../audit/AuditLogItem.vue";
import { parseAuditPayload, type AuditEventRow } from "../../utils/audit";

const props = defineProps<{
  row: AuditEventRow;
}>();

/** True when this row should render with the red
 *  `audit-item--critical` left border:
 *  - `tool_executed.exit_code != null && != 0` (failure)
 *  - any future kind that carries `critical: true`
 *  Defensive: only computes when the parsed payload shape
 *  exposes the relevant field; default false. */
const isCritical = computed<boolean>(() => {
  const parsed = parseAuditPayload(props.row.kind, props.row.payloadJson);
  if (parsed.kind === "tool_executed") {
    const ec = parsed.payload.exit_code;
    return typeof ec === "number" && Number.isFinite(ec) && ec !== 0;
  }
  if (parsed.kind === "tool") {
    return parsed.payload.critical === true;
  }
  return false;
});
</script>

<template>
  <div
    class="trace-event-item"
    :class="{ 'trace-event-item--critical': isCritical }"
  >
    <AuditLogItem :row="row" />
  </div>
</template>

<style scoped>
/* The wrapper carries the critical border so we don't fork
   AuditLogItem's class contract. The border replaces the
   audit-item's default `border-left-color: transparent` —
   AuditLogItem sets the class on its own root <li>, but
   we re-apply it on the wrapper for trace-viewer-only
   tool_executed failures. */
.trace-event-item--critical {
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
  margin-left: -3px; /* keep alignment with other rows */
}
</style>
