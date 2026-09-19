// lib/api.mjs — daemon HTTP client(零依赖;fetch + AbortSignal 超时)。
//
// 约定(照抄 scripts/group-chat-run.mjs):
// - 全 POST snake_case,唯一 GET 是 health;
// - daemon 不可达的错误文案必须带 OS 错误翻译(EPERM → "Operation not
//   permitted"):沙箱分类器 sandbox::classify_block 按该字面串触发,嵌套
//   消费时外层 shell 靠它脱沙箱重跑(2026-09-06 live 实证);并附
//   ./scripts/daemon.sh bg 拉起提示。
import process from 'node:process';

export const EXIT = {
  ok: 0,
  scriptError: 1, // 脚本自身错误(网络/不可达/流断/HTTP 非 2xx)
  chatError: 2, // chat kind=error 终态
  cancelled: 3, // SIGINT(cancel_chat 已发,session 保留)
  timeout: 7, // --timeout 到点(cancel_chat 已发)
  usage: 64, // 用法错误(未知命令/缺参/非法 flag)
};

/** 带 exitCode 的运行时错误(bin 统一按 exitCode 退出)。 */
export class EvlError extends Error {
  constructor(message, exitCode = EXIT.scriptError) {
    super(message);
    this.name = 'EvlError';
    this.exitCode = exitCode;
  }
}

// errno → 沙箱分类器认的字面串(sandbox/mod.rs classify_block:
// "Permission denied" / "Read-only file system" / "Operation not permitted")。
export function fetchFailDetail(e) {
  const cause = e?.cause;
  if (!cause) return e?.message || String(e);
  const code = cause.code || '';
  const table = { EPERM: 'Operation not permitted (EPERM)', EACCES: 'Permission denied (EACCES)' };
  return `${cause.message || ''}${code ? ` [${table[code] || code}]` : ''}`;
}

/**
 * 调 daemon。route 是 `/api/v1/` 之后的域路径(如 'agent/chat'、'health')。
 * verbose:请求/响应摘要打 stderr(经 verboseLog 注入,便于测试)。
 */
export async function api(base, route, opts = {}) {
  const {
    method = 'POST',
    body,
    okCodes = [200],
    timeoutMs = 30_000,
    verbose = false,
    verboseLog = () => {},
  } = opts;
  const url = `${base}/api/v1/${route}`;
  if (verbose) {
    verboseLog(`→ ${method} ${url}${method === 'GET' ? '' : ` ${JSON.stringify(body ?? {}).slice(0, 200)}`}`);
  }
  let res;
  try {
    res = await fetch(url, {
      method,
      headers:
        method === 'GET'
          ? { accept: 'application/json' }
          : { 'Content-Type': 'application/json' },
      body: method === 'GET' ? undefined : JSON.stringify(body ?? {}),
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    if (e?.name === 'TimeoutError' || e?.name === 'AbortError') {
      throw new EvlError(`daemon 请求超时(${method} ${url},${timeoutMs}ms)`);
    }
    throw new EvlError(
      `daemon 不可达(${url}):${fetchFailDetail(e)};先确认 daemon 在跑(./scripts/daemon.sh bg)`
    );
  }
  if (verbose) verboseLog(`← HTTP ${res.status} ${url}`);
  if (!okCodes.includes(res.status)) {
    const text = (await res.text()).slice(0, 300);
    throw new EvlError(`${method} /api/v1/${route} → HTTP ${res.status}: ${text}`);
  }
  if (res.status === 204) return null;
  const contentType = res.headers.get('content-type') || '';
  if (!contentType.includes('json')) return res.text();
  return res.json();
}
