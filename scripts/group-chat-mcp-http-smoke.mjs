#!/usr/bin/env node
// group-chat-mcp-http-smoke.mjs — daemon `/mcp`(streamable-HTTP)冒烟:
// SDK 真客户端(StreamableHTTPClientTransport)直连 daemon 验收收敛端点。
//
// 与 stdio 冒烟(group-chat-mcp-smoke.mjs)的分工:那个 spawn 子进程验 JS 壳,
// 这个走 HTTP 验 daemon 内嵌实现(routes/mcp.rs,任务 09-14-gce-mcp-daemon-converge)。
//
// 前置:daemon 在跑(本脚本就是 daemon 的 HTTP 客户端,连不上直接 FAIL 退场)。
// 非 live(默认):initialize 握手(版本协商 + serverInfo)→ ping 环回 →
// tools/list(8 工具 + wire 预算)→ 未知工具 -32602 → discussion_status
// (不存在 id)语义错误链 → wait_seconds=0 越界拒 → list_presets(内置四 key +
// degraded=false——HTTP 场 daemon 必在,可严格断言)→ list_models → 传输级
// 探针 raw fetch(GET 405 / DELETE 200 / 缺 Accept 406 / 错 Content-Type 415)。
//
// --live(烧真 token,按需):start(arch 小阵容)→ wait_seconds=30 长轮询 →
// result 全链(AC1 的 headless 回归选项;正式门禁是 ZCode 宿主实跑)。
//
// 用法:node scripts/group-chat-mcp-http-smoke.mjs [--live]
// 基址:EVERLASTING_BASE 覆盖(默认 http://127.0.0.1:7456),探 /mcp。
import process from 'node:process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPTS = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.EVERLASTING_BASE || 'http://127.0.0.1:7456';
const URL_MCP = `${BASE}/mcp`;
const BUDGET = 4200; // 与 routes/mcp.rs TOOLS_BUDGET_CHARS 同值;Rust 侧实测 3600(serde_json 序列化口径)

const live = process.argv.includes('--live');

const { Client } = await import('@modelcontextprotocol/sdk/client/index.js');
const { StreamableHTTPClientTransport } = await import('@modelcontextprotocol/sdk/client/streamableHttp.js');

const fail = (msg) => { console.error(`FAIL: ${msg}`); process.exitCode = 1; };

let client;
try {
  client = new Client({ name: 'group-chat-mcp-http-smoke', version: '1.0.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(URL_MCP)));
} catch (e) {
  console.error(`FAIL: 连不上 ${URL_MCP}:${e.message}(先 ./scripts/daemon.sh start)`);
  process.exit(1);
}
process.stderr.write(`[smoke] SDK 客户端握手 ok:${URL_MCP}\n`);

try {
  // 0) initialize 回执:握手通过 = 版本协商成立(echo 策略);锁 serverInfo。
  const info = client.getServerVersion();
  if (info?.name !== 'everlasting-group-chat') fail(`serverInfo.name 期望 everlasting-group-chat 实得 ${info?.name}`);

  // 0a) ping:initialize 之外最廉价的请求/响应环回。
  await client.ping();
  process.stderr.write('[smoke] ping ok\n');

  // 1) tools/list:八工具 + wire 预算(与 stdio 壳同锁;宿主注入 context 的地面真值)
  const { tools } = await client.listTools();
  const names = tools.map((t) => t.name);
  const want = ['start_discussion', 'discussion_status', 'discussion_result', 'cancel_discussion', 'interrupt_discussion', 'inject_message', 'list_models', 'list_presets'];
  if (JSON.stringify(names) !== JSON.stringify(want)) fail(`tools/list 期望 ${want} 实得 ${names}`);
  const chars = JSON.stringify(tools.map(({ name, description, inputSchema }) => ({ name, description, inputSchema }))).length;
  process.stderr.write(`[smoke] tools/list ok(8);wire schema ${chars} chars ≈ ${Math.round(chars / 4)} tokens(预算 ${BUDGET})\n`);
  if (chars > BUDGET) fail(`wire 预算超支:${chars} > ${BUDGET}`);

  // 1a) 未知工具:JSON-RPC error -32602(SDK 客户端按异常抛)
  try {
    await client.callTool({ name: 'no_such_tool', arguments: {} });
    fail('未知工具应报 JSON-RPC error');
  } catch (e) {
    if (e.code !== -32602) fail(`未知工具期望 -32602 实得 ${e.code}(${e.message})`);
    else process.stderr.write('[smoke] 未知工具 -32602 ok\n');
  }

  // 2) handler 链:不存在的 session → 语义错误(isError text;HTTP 场 daemon
  // 必在,错误不含 daemon 自救提示,严格断言「session 不存在」)
  const probe = await client.callTool({ name: 'discussion_status', arguments: { session_id: 'smoke-nonexistent' } });
  const probeError = JSON.parse(probe.content?.[0]?.text || '{}').error || '';
  if (!probe.isError) fail('discussion_status(不存在 id)应走 isError');
  if (!/session 不存在/.test(probeError)) fail(`错误文案不可操作:${probe.content?.[0]?.text}`);
  process.stderr.write('[smoke] handler 链 ok(session 不存在)\n');

  // 2a) wait_seconds 越界:0 应被 1-30 有界校验拒(基线报错,不进等待循环)
  const bad = await client.callTool({ name: 'discussion_status', arguments: { session_id: 'smoke-nonexistent', wait_seconds: 0 } });
  const badError = JSON.parse(bad.content?.[0]?.text || '{}').error || '';
  if (!bad.isError || !/wait_seconds/.test(badError)) fail(`wait_seconds=0 应被有界校验拒:${bad.content?.[0]?.text}`);
  else process.stderr.write('[smoke] wait_seconds 越界拒 ok\n');

  // 2b) list_presets:内置四 key 恒在 + degraded 严格 false(数据源就是本
  // daemon,无降级态可言)。用户行数量随环境漂移,不断言。
  const lp = await client.callTool({ name: 'list_presets', arguments: {} });
  if (lp.isError) fail(`list_presets 应成功:${lp.content?.[0]?.text}`);
  else {
    const payload = JSON.parse(lp.content[0].text);
    const keys = payload.presets.map((p) => p.key);
    for (const k of ['review', 'fe_review', 'arch', 'retro']) {
      if (!keys.includes(k)) fail(`list_presets 缺内置 key "${k}"`);
    }
    if (payload.degraded !== false) fail(`degraded 期望严格 false 实得 ${payload.degraded}`);
    process.stderr.write(`[smoke] list_presets ok(${payload.presets.length} 档;degraded=false)\n`);
  }

  // 2c) list_models:数组形状(id/name 恒在;数量随环境漂移,不断言)
  const lm = await client.callTool({ name: 'list_models', arguments: {} });
  if (lm.isError) fail(`list_models 应成功:${lm.content?.[0]?.text}`);
  else {
    const payload = JSON.parse(lm.content[0].text);
    if (!Array.isArray(payload.models) || payload.models.length === 0) fail('list_models 应返回非空 models 数组');
    else if (payload.models.some((m) => !m.id || !m.name)) fail('list_models 条目缺 id/name');
    else process.stderr.write(`[smoke] list_models ok(${payload.models.length} 个)\n`);
  }

  // 3) 传输级探针(raw fetch,不经 SDK):极简无状态 profile 的四道门。
  const probeHttp = async (label, init, wantStatus) => {
    const res = await fetch(URL_MCP, init);
    if (res.status !== wantStatus) fail(`${label}:期望 HTTP ${wantStatus} 实得 ${res.status}`);
    else process.stderr.write(`[smoke] ${label} ok(${wantStatus})\n`);
  };
  await probeHttp('GET(不开 SSE 流)', { method: 'GET' }, 405);
  await probeHttp('DELETE(无 session 可终结)', { method: 'DELETE' }, 200);
  await probeHttp('POST 缺 Accept 双声明', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' }) }, 406);
  await probeHttp('POST 错 Content-Type', { method: 'POST', headers: { Accept: 'application/json, text/event-stream', 'Content-Type': 'text/plain' }, body: '{}' }, 415);

  if (!live) {
    console.log('SMOKE PASS (non-live):握手 + ping + tools/list + 预算 + 错误链 + list_presets/models + 传输探针');
  } else {
    // 4) 全链:小阵容真跑(arch = moderator + 2 人;5-15 分钟,数十万 token);
    // 轮询用 wait_seconds=30 长轮询(有变化即返,无变化到点 wait_timed_out 再续)
    const started = await client.callTool({
      name: 'start_discussion',
      arguments: {
        topic: '冒烟验收:请用一句话各自陈述「transcript 落盘位置应包含时间戳」的理由,主持人直接收官。',
        cwd: path.resolve(SCRIPTS, '..'), // 讨论 cwd = 仓库根(转录落 <cwd>/out/,与 M1/stdio 同处)
        preset: 'arch',
      },
    });
    if (started.isError) fail(`start 失败:${started.content?.[0]?.text}`);
    const { session_id } = JSON.parse(started.content[0].text);
    process.stderr.write(`[smoke] started ${session_id};wait_seconds=30 长轮询…\n`);
    let last;
    const t0 = Date.now();
    while (Date.now() - t0 < 15 * 60_000) {
      const st = await client.callTool({ name: 'discussion_status', arguments: { session_id, wait_seconds: 30 } });
      last = JSON.parse(st.content[0].text);
      process.stderr.write(`[smoke] poll: busy=${last.busy} stop_reason=${last.stop_reason ?? '-'} msgs=${last.messages ?? '-'} elapsed=${last.elapsed_s ?? '-'}s${last.wait_timed_out ? ' (wait 超时再续)' : ''}\n`);
      if (!last.busy && last.stop_reason) break;
    }
    if (!last?.stop_reason) { fail('15 分钟未收官(超时)'); }
    else {
      const res = await client.callTool({ name: 'discussion_result', arguments: { session_id } });
      const payload = JSON.parse(res.content[0].text);
      if (res.isError) fail(`result 失败:${res.content?.[0]?.text}`);
      console.log(`SMOKE PASS (live):stop_reason=${payload.stop_reason} messages=${payload.stats?.messages} transcript=${payload.transcript_path}`);
    }
  }
} catch (e) {
  fail(e.stack || e.message);
} finally {
  await client.close().catch(() => {});
}
