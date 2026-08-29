// useCodeBlockCopy — CH4-5 (2026-08-29): delegated copy handler for the
// fenced-code-block chrome that `utils/markdown.ts` emits into v-html.
//
// The chrome lives inside a v-html binding, so its nodes can't carry
// Vue listeners — each markdown container binds this ONE click handler
// on its root and the button is located via the `data-code-copy` /
// `data-code-block` hooks the renderer stamps (see markdown.ts).
//
// Clipboard policy mirrors `<CodeBlockPrimitive>`: `navigator.clipboard
// .writeText` + a 2s "已复制" ack, silent on failure (the API throws
// outside a secure context; Tauri runs under https/file so this is
// defensive).
export function useCodeBlockCopy() {
  async function onMarkdownClick(e: MouseEvent): Promise<void> {
    const target = e.target;
    if (!(target instanceof Element)) return;
    const btn = target.closest<HTMLElement>("[data-code-copy]");
    if (!btn) return;
    const code =
      btn.closest<HTMLElement>("[data-code-block]")?.querySelector("pre code")
        ?.textContent ?? "";
    try {
      await navigator.clipboard.writeText(code);
      btn.textContent = "已复制";
      setTimeout(() => {
        btn.textContent = "复制";
      }, 2000);
    } catch {
      // clipboard unavailable (non-secure context) → silent
    }
  }
  return { onMarkdownClick };
}
