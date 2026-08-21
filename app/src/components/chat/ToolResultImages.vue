<!-- ToolResultImages — 08-21-b1-image-followups R6: tool-returned
     images (read_file on an image) rendered as a thumbnail row inside
     the tool-result card. Served by the existing attachments GET
     route (pwa-remote proxy form included via `attachmentUrl`). -->
<script setup lang="ts">
import { computed } from "vue";
import type { ToolResultImageRef } from "../../stores/chat.types";
import { attachmentUrl } from "../../utils/attachmentUrl";

const props = defineProps<{
  images: ToolResultImageRef[];
  sessionId: string;
}>();

const urls = computed(() =>
  props.sessionId
    ? props.images.map((img) => ({
        url: attachmentUrl(props.sessionId, img.file),
        label: img.media_type,
        tokens: img.tokens_est,
      }))
    : [],
);
</script>

<template>
  <div v-if="urls.length" class="tool-result-images">
    <a
      v-for="(u, i) in urls"
      :key="i"
      class="tool-result-images__item"
      :href="u.url"
      target="_blank"
      rel="noopener"
      :title="`查看图片${u.tokens !== undefined ? `(估算 ${u.tokens} tokens)` : ''}`"
    >
      <img :src="u.url" :alt="`工具返回图片 ${i + 1}`" loading="lazy" />
    </a>
  </div>
</template>

<style scoped>
.tool-result-images {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 6px;
}

.tool-result-images__item {
  display: block;
  width: 64px;
  height: 64px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
  overflow: hidden;
  background: var(--color-bg-elevated);
}

.tool-result-images__item img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
</style>
