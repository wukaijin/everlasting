// useCodeBlockCopy — v-html markdown 容器的委托交互层(文件名是
// 首个交互"代码复制"的历史名,职责已扩,导出保持零调用点改动):
//
//   - CH4-5 (2026-08-29):围栏代码块复制按钮(data-code-copy /
//     data-code-block,markdown.ts 的 code renderer 注入);
//   - 09-13 图片路径预览:a[data-image-path](markdown.ts 的
//     linkifyImagePaths 产物),点击开 ImageViewerModal 弹层。
//
// 交互都活在 v-html 绑定里,节点带不了 Vue 监听器 —— 每个 markdown
// 容器在自己的根上绑这一个 click handler,内部用 `closest` 按
// data-* 钩子定位。复制策略同 `<CodeBlockPrimitive>`:
// `navigator.clipboard.writeText` + 2s "已复制" ack,失败静默
// (该 API 在非安全上下文会 throw;Tauri 跑在 https/file 下,属防御)。
//
// 图片路径分支故意不在此处解析 cwd —— open() 只传原始路径,解析在
// ImageViewerModal 内做(见 useImageViewer 的模块注释)。
import { useImageViewer } from "./useImageViewer";

export function useCodeBlockCopy() {
  const imageViewer = useImageViewer();

  async function onMarkdownClick(e: MouseEvent): Promise<void> {
    const target = e.target;
    if (!(target instanceof Element)) return;
    const imgLink = target.closest<HTMLAnchorElement>("a[data-image-path]");
    if (imgLink) {
      // data-image-path 由我们自己注入(非用户可控 href);链接形态的
      // href 导航(相对路径会打穿 SPA 路由)一律拦下走弹层。
      e.preventDefault();
      const raw = imgLink.dataset.imagePath;
      if (raw) imageViewer.open(raw);
      return;
    }
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
