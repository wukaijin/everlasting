// lib/sse.mjs — SSE 手解(design §3:Node 20 无全局 EventSource,fetch +
// ReadableStream;零依赖,不用 eventsource)。
//
// `parseSseChunk` 是纯函数(供 node --test):输入累计 buffer,按 "\n\n" 切帧,
// 输出 {frames, rest};跨 chunk 边界的不完整尾部留在 rest。帧内解析
// `event:` / `data:`(data 可多行,以 \n 拼接)/ `id:` / `retry:`(忽略)/
// `:` 开头注释行(忽略);冒号后单个空格按规范剥离;CRLF 容忍。
//
// `connectSse` 是**立即连接**(非惰性 generator):RULE-SMOKE-001 要求订阅先于
// agent/chat 建立——惰性 async generator 首次 next() 才 fetch,若先 POST 再
// for-await 就会迟挂漏事件,所以这里把连接从迭代里拆出来。
import { fetchFailDetail } from './api.mjs';

/** 切帧 + 解析。data 尽量 JSON.parse,失败原样字符串。无 data 的帧丢弃。 */
export function parseSseChunk(buffer) {
  const normalized = buffer.replace(/\r\n/g, '\n');
  const parts = normalized.split('\n\n');
  const rest = parts.pop() ?? '';
  const frames = [];
  for (const raw of parts) {
    const frame = parseSseFrame(raw);
    if (frame) frames.push(frame);
  }
  return { frames, rest };
}

function parseSseFrame(frameText) {
  let event = 'message';
  const dataLines = [];
  for (const line of frameText.split('\n')) {
    if (line === '' || line.startsWith(':')) continue; // 空行 / 注释(keepalive)
    if (line.startsWith('event:')) {
      event = line.slice('event:'.length).replace(/^ /, '');
    } else if (line.startsWith('data:')) {
      dataLines.push(line.slice('data:'.length).replace(/^ /, ''));
    } else if (line.startsWith('id:') || line.startsWith('retry:')) {
      // MVP 不做重连续读(design §8):id/retry 忽略
    }
  }
  if (dataLines.length === 0) return null;
  const dataRaw = dataLines.join('\n');
  let data;
  try {
    data = JSON.parse(dataRaw);
  } catch {
    data = dataRaw;
  }
  return { event, data };
}

/**
 * 立即建立 SSE 连接(GET /api/v1/stream),返回 async iterator 吐 {event, data}。
 * 连接失败翻译 OS 错误 + daemon.sh 提示(与 api.mjs 同约定);abort(signal)
 * 后迭代器静默收尾(不抛)。
 */
export async function connectSse(base, { signal } = {}) {
  const url = `${base}/api/v1/stream`;
  let res;
  try {
    res = await fetch(url, { headers: { accept: 'text/event-stream' }, signal });
  } catch (e) {
    if (e?.name === 'AbortError') {
      return (async function* () {})();
    }
    throw new Error(
      `SSE 订阅失败(${url}):${fetchFailDetail(e)};先确认 daemon 在跑(./scripts/daemon.sh bg)`
    );
  }
  if (!res.ok || !res.body) {
    throw new Error(`SSE 订阅失败:HTTP ${res.status} ${url}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();

  async function* events() {
    let buffer = '';
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const { frames, rest } = parseSseChunk(buffer);
        buffer = rest;
        for (const frame of frames) yield frame;
      }
      // 冲掉 decoder 尾字节;末尾无换行的半帧按完整帧尽力解析
      buffer += decoder.decode();
      if (buffer !== '') {
        const { frames } = parseSseChunk(`${buffer}\n\n`);
        for (const frame of frames) yield frame;
      }
    } catch (e) {
      if (e?.name === 'AbortError') return; // 主动收线(终态/SIGINT/超时)
      throw e;
    } finally {
      try {
        await reader.cancel();
      } catch {
        // 连接已在对面关闭等场景的 cancel 报错无意义
      }
    }
  }
  return events();
}
