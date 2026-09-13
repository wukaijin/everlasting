// useImageViewer — 图片路径预览弹层(2026-09-13)的全局开闭状态。
//
// 为什么不是 pinia store:状态只有一开一关一个字符串(当前预览的原始
// 路径),无 getter 族、无持久化、无跨 store 依赖 —— pinia 的收益
// (devtools/插件/SSR)对这个体量是纯开销。模块级 reactive 单例 +
// composable 包装即可,组织方式与 useToast 的"模块级状态"先例一致。
//
// 持有的是 linkify 捕获的**原始**路径(可能 `out/x.png` 相对形态):
// 相对路径的解析基准是"点击那一刻"的会话 cwd,渲染时刻的 cwd 会随后
// 续会话切换漂移,所以解析推迟到 ImageViewerModal 内(chromeStore/
// chatStore 在组件 setup 里安全可取)。
import { computed, reactive } from "vue";

const state = reactive<{ path: string | null }>({ path: null });

export function useImageViewer() {
  const isOpen = computed(() => state.path !== null);
  return {
    isOpen,
    /** linkify 的原始路径(未解析 cwd),null = 关闭。 */
    path: computed(() => state.path),
    /** 打开预览。`rawPath` 是 data-image-path 原文。 */
    open(rawPath: string): void {
      state.path = rawPath;
    },
    close(): void {
      state.path = null;
    },
  };
}
