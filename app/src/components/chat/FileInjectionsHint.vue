<script setup lang="ts">
// FileInjectionsHint — B2 PR3: the hint row rendered under a
// user message bubble, listing every `@relpath` token the
// agent loop saw and what it did with each one (text
// injection line count, non-text degradation, or skip
// reason). The user gets a glance "LLM saw this" view
// without scrolling to the assistant's reply.
//
// Data flow:
//   - The live path: the agent loop's `ChatEvent::FileInjections`
//     fires the controller's `case "file_injections"` arm,
//     which patches the user message's `injections` array
//     in place. The next Vue render shows the hint row.
//   - The reload path: `rehydrateMessages` parses
//     `MessageRow.metadata` JSON into the same
//     `injections` shape (see
//     `streamController.ts::rehydrateMessages`).
//
// The component is a thin renderer: the parent
// (`MessageItem.vue`) decides when to mount it
// (`v-if="message.role === 'user' && message.injections?.length"`),
// the component just iterates the manifest and emits the
// per-row markup. No event handling, no async, no
// re-fetch — the data is whatever the controller has on
// the user message at render time.

import { computed } from "vue";
import type { InjectionEntry } from "../../stores/chat.types";

const props = defineProps<{
  injections: InjectionEntry[];
}>();

// Each row is a { path, glyph, suffix } triple. The
// suffix carries the human-readable status text. Status
// glyphs:
//   - `✓`  injected (text file content went into the
//          LLM context)
//   - `⊘`  not injected (placeholder OR skip; the
//          suffix disambiguates)
//
// The mapping mirrors the prd's Decision section
// (https://.../06-17-b2-pr3-at-file-injection-hint/prd.md):
//
//   · src/foo.ts   ✓ 注入 48 行
//   · bar.png      ⊘ 图片·未注入(B1)
//   · spec.docx    ⊘ 文档·未注入(可 pandoc 转换)
//   · missing.txt  ⊘ 跳过(不存在)
//
// The frontend maps the snake_case enum to the Chinese
// labels here (the backend's `fileKind.label()` /
// `SkipReason.label()` are not exposed on the wire —
// keeping the wire enum-shaped means future variants
// don't need a frontend release to land).
type Row = {
  path: string;
  glyph: "ok" | "bad";
  status: string;
};

const rows = computed<Row[]>(() => {
  return props.injections.map((entry) => {
    const a = entry.action;
    if (a.kind === "injected") {
      return {
        path: entry.path,
        glyph: "ok",
        status: `注入 ${a.lines} 行`,
      };
    }
    if (a.kind === "degraded") {
      const fileLabel =
        a.file_kind === "image"
          ? "图片"
          : a.file_kind === "pdf"
            ? "PDF"
            : a.file_kind === "office"
              ? "文档"
              : a.file_kind === "binary"
                ? "二进制"
                : "文件";
      // The "未注入" hint copy mirrors the backend's
      // `expand_for_kind` placeholder wording — see
      // `app/src-tauri/src/agent/at_file.rs`. The
      // "(B1)" tag is image-specific; the
      // pandoc/pdftotext hint is for office/pdf.
      const hint =
        a.file_kind === "image"
          ? "未注入(B1)"
          : a.file_kind === "office"
            ? "未注入(可 pandoc 转换)"
            : a.file_kind === "pdf"
              ? "未注入(可 pdftotext 转换)"
              : "未注入(二进制)";
      return {
        path: entry.path,
        glyph: "bad",
        status: `${fileLabel}·${hint}`,
      };
    }
    // Skipped
    const reasonLabel =
      a.reason === "out_of_root"
        ? "越界"
        : a.reason === "missing"
          ? "不存在"
          : a.reason === "unreadable"
            ? "不可读"
            : "未知";
    return {
      path: entry.path,
      glyph: "bad",
      status: `跳过(${reasonLabel})`,
    };
  });
});
</script>

<template>
  <div class="file-injections-hint" role="note">
    <div class="file-injections-hint__title">📎 已引用文件:</div>
    <ul class="file-injections-hint__list">
      <li
        v-for="(row, idx) in rows"
        :key="`${row.path}-${idx}`"
        class="file-injections-hint__row"
      >
        <span class="file-injections-hint__bullet">·</span>
        <span class="file-injections-hint__path">{{ row.path }}</span>
        <span
          :class="[
            'file-injections-hint__status',
            row.glyph === 'ok'
              ? 'file-injections-hint__status--ok'
              : 'file-injections-hint__status--bad',
          ]"
        >
          <span aria-hidden="true">{{ row.glyph === "ok" ? "✓" : "⊘" }}</span>
          {{ row.status }}
        </span>
      </li>
    </ul>
  </div>
</template>

<style scoped>
/* B2 PR3 hint row — secondary color so it doesn't
   fight the user bubble for attention, monospace path
   to keep the `@relpath` columns aligned across rows,
   ✓/⊘ status glyphs in green/red for at-a-glance
   "injected vs not" scanning. The container mirrors
   the `.msg__tools` flex-column pattern from
   `MessageItem.vue` (max-width 100%, gap 2px). */
.file-injections-hint {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-top: 4px;
  padding: 6px 10px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  max-width: 100%;
  word-break: break-all;
}

.file-injections-hint__title {
  font-weight: var(--weight-semibold);
  color: var(--color-text-muted);
  font-size: var(--text-xs);
  margin-bottom: 2px;
}

.file-injections-hint__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.file-injections-hint__row {
  display: flex;
  align-items: baseline;
  gap: 6px;
  line-height: 1.4;
}

.file-injections-hint__bullet {
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.file-injections-hint__path {
  color: var(--color-text-primary);
  flex-shrink: 0;
  /* Monospace so two long paths column-align naturally;
     the bullet + status also align across rows. */
  font-family: var(--font-mono);
  word-break: break-all;
}

.file-injections-hint__status {
  margin-left: auto;
  white-space: nowrap;
  flex-shrink: 0;
}

.file-injections-hint__status--ok {
  color: var(--color-status-success, #4ade80);
}

.file-injections-hint__status--bad {
  color: var(--color-status-warning, #f59e0b);
}
</style>
