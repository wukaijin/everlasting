<script setup lang="ts">
// MessageImages — B1 (2026-08-16) image-multimodal R2a: the
// horizontal thumbnail strip rendered under a user message bubble
// (same level as `FileInjectionsHint`), one 64px square per image
// the turn carried (pasted uploads + @-file image injections).
//
// Data flow: `MessageItem.vue` reads `message.metadata.attachments`
// (the manifest `chatSendActions.send` writes optimistically and
// the agent loop rewrites on rehydrate — see `AttachmentView` in
// `chat.types.ts`) and maps it into the entry shape below. The
// component itself is a thin renderer:
//   - an entry with a server `file` name resolves through
//     `attachmentUrl(sessionId, file)` (the daemon GET route —
//     works both right after send, since upload completes before
//     the optimistic row is inserted, and post-rehydrate);
//   - an entry with only a `localUrl` (blob objectURL) renders
//     directly from it — defensive fallback for the pre-upload
//     window.
// Clicking a thumbnail opens the full-size image in a new tab
// (`window.open`) — no lightbox dependency by design (零新依赖).

import { attachmentUrl } from "../../utils/attachmentUrl";

/** One renderable attachment ref. `file` is the server-generated
 *  attachment name (post-upload / rehydrated); `localUrl` is the
 *  blob objectURL (optimistic). At least one is set — the parent
 *  filters entries with neither. */
interface ImageRef {
  file?: string;
  localUrl?: string;
  mediaType: string;
}

const props = defineProps<{
  sessionId: string;
  images: ImageRef[];
}>();

/** Display / open URL for one entry. `file` wins when present —
 *  the daemon route outlives the blob (which is revoked once the
 *  staged strip is cleared) and is what rehydrated rows carry. */
function urlFor(img: ImageRef): string | undefined {
  if (img.file) return attachmentUrl(props.sessionId, img.file);
  return img.localUrl;
}

function openImage(img: ImageRef): void {
  const url = urlFor(img);
  if (url) window.open(url);
}
</script>

<template>
  <div class="message-images" role="group" aria-label="图片附件">
    <button
      v-for="(img, idx) in images"
      :key="img.file ?? img.localUrl ?? idx"
      type="button"
      class="message-images__item"
      :data-testid="`message-image-${idx}`"
      :title="img.mediaType"
      @click="openImage(img)"
    >
      <img
        v-if="urlFor(img)"
        class="message-images__thumb"
        :src="urlFor(img)"
        :alt="img.mediaType"
        loading="lazy"
      />
    </button>
  </div>
</template>

<style scoped>
/* B1 R2a: thumbnail strip — mirrors the ChatInput staging strip's
   pattern (horizontal, wrapping, small rounded squares) but with
   64px read-only cells: the message row is a record, not an edit
   surface, so the cells can be larger than the 48px staging thumbs.
   The row sits inside the user message's right-aligned flex
   column; `align-self: flex-end` keeps it tucked under the bubble
   instead of stretching to the li's full width. */
.message-images {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 4px;
  align-self: flex-end;
  max-width: 100%;
}

.message-images__item {
  width: 64px;
  height: 64px;
  padding: 0;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-elevated);
  overflow: hidden;
  cursor: pointer;
  transition: border-color var(--duration-fast) var(--ease-out);
}

.message-images__item:hover,
.message-images__item:focus-visible {
  border-color: var(--color-accent);
}

.message-images__thumb {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
</style>
