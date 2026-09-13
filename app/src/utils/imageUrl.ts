// imageUrl — 图片路径预览(2026-09-13):聊天 markdown 里识别出的本地
// 图片路径(`utils/markdown.ts` linkify 产物,`data-image-path` 原文)
// 点击后弹层 `<img>` 的取数 URL + 相对路径解析。
//
// 路由:daemon `GET /api/v1/files/image?path=<abs|~前缀>`
// (`daemon/routes/files.rs`)。扩展白名单 + 32 MiB 上限在 daemon 侧;
// svg 有意不在白名单(独立文档打开时脚本会跑),前端不做二次校验 ——
// 后端是唯一闸门,前端名单必然后漂移。
//
// URL 三传输模式与 `attachmentUrl.ts` 同构(设计 §3.3):
//   - browser-local PROD: daemonBase() === location.origin → 同源绝对
//   - DEV 浏览器(1420): 跨源绝对 URL
//   - pwa-remote(device token 存在): 走 remote proxy catch-all +
//     `?access_token=` query 鉴权(同 SSE/attachment 的 GET 通道)
//
// `path` 形态契约:进 `imageUrl` 前必须先过 `resolveImagePath` 变成
// 绝对路径或 `~/` 前缀(daemon 不接受相对路径,会话 cwd 只有前端知道)。

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
  const base = daemonBase().replace(/\/+$/, "");
  const p = encodeURIComponent(path);
  // 08-26 多节点:与 invoke/SSE 同源,取"当前选中节点"的 token。
  const token = currentDeviceToken();
  if (token) {
    // pwa-remote: proxy + query token(GET binary pass-through)。
    // path 本身也在 query 里,encodeURIComponent 已防 `&`/`=` 串位。
    return `${base}/api/v1/proxy/api/v1/files/image?path=${p}&access_token=${encodeURIComponent(token)}`;
  }
  return `${base}/api/v1/files/image?path=${p}`;
}
