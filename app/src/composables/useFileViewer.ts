// useFileViewer — 文件路径预览弹层(2026-09-13)的全局开闭状态与取数。
//
// 镜像 useImageViewer 的模块级单例(为什么不是 pinia store:同图片版
// —— 单开一关 + 一段文本的状态体量,pinia 收益是纯开销)。
//
// 与图片版的一个结构性差异:pdf **不开弹层**,open() 在点击时刻就要拿
// 取数 URL 去 window.open(浏览器原生 viewer)。所以 cwd 解析(图片版
// 推迟到弹层 computed 里做)在这里提前到 open() 内 —— useChatStore 在
// 点击事件处理器里调用,彼时 pinia 已随 app 安装,合法;点击时刻的
// currentCwd 也正是语义正确的解析基准(见 useImageViewer 模块注释)。
//
// 取数:文本类 fetch(`/files/raw`,fileUrl 三传输模式同 imageUrl)→
// status: loading → ok/error。错误不区分 400/404/413 —— 弹层统一文案 +
// 「新标签打开」兜底(ImageViewerModal 同款模式)。竞态防护:连续 open
// 时旧 fetch 以序号作废,慢响应不回写新路径的状态。
import { computed, reactive } from "vue";
import { useChatStore } from "../stores/chat";
import { fileUrl, resolveImagePath } from "../utils/imageUrl";

export type FileViewerStatus = "idle" | "loading" | "ok" | "error";
/** 渲染分派:`.md`/`.markdown` 走 markdown 管线,其余文本扩展走代码高亮卡。 */
export type FileViewerMode = "markdown" | "code";

interface FileViewerState {
  /** linkify 原文(未解析 cwd;null = 关闭,镜像 useImageViewer.path)。 */
  rawPath: string | null;
  /** 点击时刻按会话 cwd 解析出的 daemon 可接受形态。 */
  absPath: string;
  /** 小写扩展名(渲染分派 + CodeBlockPrimitive 的 language 别名)。 */
  ext: string;
  status: FileViewerStatus;
  content: string;
}

const state = reactive<FileViewerState>({
  rawPath: null,
  absPath: "",
  ext: "",
  status: "idle",
  content: "",
});

const MARKDOWN_EXTS = new Set(["md", "markdown"]);

/** 从路径取小写扩展名;无扩展名 → 空串(daemon 白名单外,fetch 必 400)。 */
function extOf(path: string): string {
  const dot = path.lastIndexOf(".");
  const slash = path.lastIndexOf("/");
  if (dot < 0 || dot < slash) return "";
  return path.slice(dot + 1).toLowerCase();
}

// 连续 open 的竞态防护:只认最新一次 open 发起的响应。
let fetchSeq = 0;

async function openInner(rawPath: string): Promise<void> {
  const chatStore = useChatStore();
  const resolved = resolveImagePath(rawPath, chatStore.currentCwd);
  // pdf 分派先于弹层:浏览器原生 viewer,不占弹层状态(prd Q1 决议)。
  const ext = extOf(resolved);
  if (ext === "pdf") {
    window.open(fileUrl(resolved), "_blank", "noreferrer");
    return;
  }
  const seq = ++fetchSeq;
  state.rawPath = rawPath;
  state.absPath = resolved;
  state.ext = ext;
  state.status = "loading";
  state.content = "";
  try {
    const res = await fetch(fileUrl(resolved));
    if (seq !== fetchSeq) return; // 已被更新的 open 抢占,丢弃旧响应
    if (!res.ok) {
      // daemon 400(非白名单/相对路径)/404/413 → 同一错误态。
      state.status = "error";
      return;
    }
    const text = await res.text();
    if (seq !== fetchSeq) return;
    state.content = text;
    state.status = "ok";
  } catch {
    if (seq !== fetchSeq) return;
    // 网络失败 / daemon 不在:与 HTTP 错误同一错误态兜底。
    state.status = "error";
  }
}

export function useFileViewer() {
  return {
    isOpen: computed(() => state.rawPath !== null),
    /** linkify 的原始路径(未解析 cwd),null = 关闭。 */
    rawPath: computed(() => state.rawPath),
    /** 解析后的绝对形态(footer 展示 + 新标签打开共用)。 */
    absPath: computed(() => state.absPath),
    ext: computed(() => state.ext),
    status: computed(() => state.status),
    content: computed(() => state.content),
    mode: computed<FileViewerMode>(() =>
      MARKDOWN_EXTS.has(state.ext) ? "markdown" : "code",
    ),
    /** 打开预览。`rawPath` 是 data-file-path 原文;pdf 由此直接新标签。 */
    open(rawPath: string): void {
      void openInner(rawPath);
    },
    close(): void {
      fetchSeq++; // 在途响应作废,关掉后不再回写状态
      state.rawPath = null;
      state.absPath = "";
      state.ext = "";
      state.status = "idle";
      state.content = "";
    },
  };
}
