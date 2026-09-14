// imageUrl — 本地路径预览(2026-09-13 图片 + 同日文件通道)的取数 URL
// 与相对路径解析。聊天 markdown/工具输出里识别出的本地路径
// (`utils/markdown.ts` linkify 产物,`data-image-path`/`data-file-path`
// 原文)经 `resolveImagePath` 解析后,图片走 `imageUrl`(<img> 直连
// `/files/image`),其余文件走 `fileUrl`(fetch 弹层 / pdf 新标签,直连
// `/files/raw`)。
//
// 路由:daemon `GET /api/v1/files/image|raw?path=<abs|~前缀>`
// (`daemon/routes/files.rs`)。扩展白名单 + 大小上限在 daemon 侧;
// svg 有意不在图片白名单(独立文档打开时脚本会跑),`/files/raw` 的
// 文本类强制 text/plain 下发(.html/.htm 只作文本查看);前端不做二次
// 校验 —— 后端是唯一闸门,前端名单必然后漂移。
//
// URL 三传输模式与 `attachmentUrl.ts` 同构(设计 §3.3):
//   - browser-local PROD: daemonBase() === location.origin → 同源绝对
//   - DEV 浏览器(1420): 跨源绝对 URL
//   - pwa-remote(device token 存在): 走 remote proxy catch-all +
//     `?access_token=` query 鉴权(同 SSE/attachment 的 GET 通道)
//
// `path` 形态契约:进 `imageUrl`/`fileUrl` 前必须先过 `resolveImagePath`
// 变成绝对路径或 `~/` 前缀(daemon 不接受相对路径,会话 cwd 只有前端
// 知道)。

import { daemonBase } from "../transport/http";
import { currentDeviceToken } from "../transport/auth";

/** 把 linkify 捕获的原始路径解析成 daemon 可接受的形态。
 *
 *  - `/abs/...` 与 `~/...` 原样通过(`~` 由 daemon 端 `dirs::home_dir`
 *    展开 —— home 以 daemon 运行环境为准,与前端 configStore.homeDir
 *    同源,省一次前端依赖);
 *  - 相对路径(`out/x.png` / `./x.png` / `../x.png`)与会话 cwd 拼接;
 *    cwd 未知(空串,异常态)时原样透传,由弹层的 404 错误态兜底
 *    提示,不在这里吞错。
 *
 *  名字沿图片通道首版保留(调用方 grep 友好),语义是通用本地路径
 *  解析:图片弹层(既有)与文件弹层(FileViewerModal)共用。
 */
export function resolveImagePath(raw: string, cwd: string): string {
  if (raw.startsWith("/") || raw.startsWith("~/")) return raw;
  const base = cwd.replace(/\/+$/, "");
  if (!base) return raw;
  return `${base}/${raw.replace(/^\.?\/+/, "")}`;
}

/** Absolute URL for the preview image bytes. `path` must already be
 *  absolute or `~/`-prefixed (see `resolveImagePath`). */
export function imageUrl(path: string): string {
  return localFileUrl("files/image", path);
}

/** Absolute URL for the raw file bytes(`/files/raw`:文本类 text/plain
 *  + pdf)。`path` 契约同 `imageUrl`;FileViewerModal 的 fetch 与 pdf
 *  的 `window.open` 共用。 */
export function fileUrl(path: string): string {
  return localFileUrl("files/raw", path);
}

/** 存在性探针 URL(`/files/stat`:200 = 存在且是普通文件,404 = 不存在,
 *  body 空)。linkify 乐观渲染后由 `utils/pathExistence.ts` 异步确认用,
 *  渲染路径零阻塞,不挂任何弹层。`path` 契约同 `imageUrl`。 */
export function statUrl(path: string): string {
  return localFileUrl("files/stat", path);
}

/// 三传输模式共用的 URL 构造(imageUrl/fileUrl 仅路由段不同,刻意
/// 单点实现防两份漂移;原 imageUrl 内联体 09-13 文件通道时收编)。
function localFileUrl(route: string, path: string): string {
  const base = daemonBase().replace(/\/+$/, "");
  const p = encodeURIComponent(path);
  // 08-26 多节点:与 invoke/SSE 同源,取"当前选中节点"的 token。
  const token = currentDeviceToken();
  if (token) {
    // pwa-remote: proxy + query token(GET binary pass-through)。
    // path 本身也在 query 里,encodeURIComponent 已防 `&`/`=` 串位。
    return `${base}/api/v1/proxy/api/v1/${route}?path=${p}&access_token=${encodeURIComponent(token)}`;
  }
  return `${base}/api/v1/${route}?path=${p}`;
}
