// lib/mcp.mjs — daemon `/mcp` JSON-RPC client(tools/call 运输层,design §2)。
//
// 端点无状态(mcp.rs handle_request 逐请求独立分发):不 initialize,直发
// tools/call。响应三态(parseMcpResponse 纯函数,mcp.test.mjs 覆盖):
//   1. body.error → JSON-RPC 协议错(未知工具/方法 -32602/-32601)→ EvlError;
//   2. body.result.isError === true → 工具语义/infra 错:text_result 的
//      content[0].text 为 {error[, hint]}(infra 带 daemon 拉起提示)→ EvlError;
//   3. 正常 → content[0].text 是工具返回值的 pretty JSON 字符串,二次 parse。
// 传输门(mcp.rs + http-smoke 实证):Accept 须同时含 application/json 与
// text/event-stream(缺 → 406);Content-Type 须 application/json(否则 415);
// 响应 200 纯 JSON。网络错文案复用 fetchFailDetail(OS 错误翻译字面串,
// 沙箱分类器依赖;与 api.mjs 同款)。
import { EvlError, fetchFailDetail } from './api.mjs';
import { truncate } from './format.mjs';

/** 单次 tools/call 默认超时;带 wait_seconds 的长轮询由调用方给
 * wait_seconds*1000 + 15000(design §2)。 */
export const MCP_TIMEOUT_MS = 30_000;

// 进程内自增 JSON-RPC id(无状态端点不校验,但响应 id 不匹配仍按协议错报)。
let rpcIdSeq = 0;

/** JSON 预览(错误摘要用;环引用/BigInt 防御)。 */
function preview(v, max = 200) {
  try {
    return truncate(JSON.stringify(v) ?? 'null', max);
  } catch {
    return truncate(String(v), max);
  }
}

/**
 * 解析 /mcp 响应体 → 工具载荷(纯函数)。三态见文件头;malformed body
 * (缺 result / 缺 content[0].text / text 非 JSON / id 不匹配)一律按协议错
 * 抛 EvlError——协议面异常不静默吞(design §8:未来 daemon 加非 JSON text 时
 * 由这条路兜住)。
 */
export function parseMcpResponse(body, { expectId } = {}) {
  if (body == null || typeof body !== 'object' || Array.isArray(body)) {
    throw new EvlError(`MCP 响应非 JSON-RPC 对象:${preview(body)}`);
  }
  if (expectId !== undefined && body.id !== expectId) {
    throw new EvlError(`MCP 响应 id 不匹配(期望 ${expectId},实得 ${preview(body.id)})`);
  }
  if (body.error != null) {
    const { code, message } = body.error ?? {};
    throw new EvlError(`MCP 协议错 ${code ?? '?'}:${message ?? preview(body.error)}`);
  }
  const result = body.result;
  if (result == null || typeof result !== 'object') {
    throw new EvlError(`MCP 响应缺 result:${preview(body)}`);
  }
  const text = result.content?.[0]?.text;
  if (typeof text !== 'string') {
    throw new EvlError(`MCP 响应缺 content[0].text:${preview(result)}`);
  }
  if (result.isError === true) {
    // 工具语义/infra 错(daemon 不走 JSON-RPC error 通道):text 为 {error[, hint]}
    let payload = null;
    try {
      payload = JSON.parse(text);
    } catch {
      throw new EvlError(truncate(text, 300));
    }
    const hint = payload?.hint ? `(hint: ${payload.hint})` : '';
    throw new EvlError(`${payload?.error ?? truncate(text, 200)}${hint}`);
  }
  try {
    return JSON.parse(text);
  } catch (e) {
    throw new EvlError(`MCP 工具载荷非 JSON(${e.message}):${truncate(text, 200)}`);
  }
}

/**
 * 调一个 MCP 工具。opts: { base, tool, args, timeoutMs, verbose, verboseLog }。
 * 返回工具载荷(parseMcpResponse 三态:EvlError 携带协议/语义错,exitCode 1)。
 */
export async function callMcpTool({
  base,
  tool,
  args = {},
  timeoutMs = MCP_TIMEOUT_MS,
  verbose = false,
  verboseLog = () => {},
}) {
  const url = `${base}/mcp`;
  const id = ++rpcIdSeq;
  const body = { jsonrpc: '2.0', id, method: 'tools/call', params: { name: tool, arguments: args } };
  if (verbose) verboseLog(`→ mcp ${tool} ${preview(args)}`);
  let res;
  try {
    res = await fetch(url, {
      method: 'POST',
      // 双 Accept 声明是 daemon 406 探针的硬要求;Content-Type 错 → 415
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json, text/event-stream',
      },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    if (e?.name === 'TimeoutError' || e?.name === 'AbortError') {
      throw new EvlError(`mcp 请求超时(POST ${url},${timeoutMs}ms)`);
    }
    throw new EvlError(
      `daemon 不可达(${url}):${fetchFailDetail(e)};先确认 daemon 在跑(./scripts/daemon.sh bg)`
    );
  }
  if (!res.ok) {
    const text = (await res.text()).slice(0, 300);
    throw new EvlError(`POST /mcp → HTTP ${res.status}: ${text}(406/415 = Accept/Content-Type 头错,CLI bug)`);
  }
  const json = await res.json().catch((e) => {
    throw new EvlError(`MCP 响应非 JSON:${e.message}`);
  });
  const payload = parseMcpResponse(json, { expectId: id });
  if (verbose) verboseLog(`← mcp ok ${tool} ${preview(payload)}`);
  return payload;
}
