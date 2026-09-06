#!/usr/bin/env node
// group-chat-mcp-smoke.mjs — MCP server 冒烟:真 spawn stdio 进程验收。
//
// 非 live(默认,daemon 可跑可不跑):spawn server → tools/list 断言 6 工具
// + wire 预算 → callTool discussion_status(不存在 id)断言 handler 链给出
// 可操作错误(daemon 在跑 = 「session 不存在」;没跑 = daemon 提示)。
//
// --live(烧真 token,按需):对真 daemon 走「start(arch 小阵容)→ 轮询
// → result」全链。这是 AC1 的 headless 回归选项;正式门禁是 ZCode 宿主
// 实跑(implement.md Step 5 归属定案)。
//
// 用法:node scripts/group-chat-mcp-smoke.mjs [--live] [--bin <path>]
// --bin <path>:对 standalone bin(deploy 产物)冒烟,替代 node 直连源码;
// 断言链完全同构,默认行为不变(部署面验收 AC2,任务 09-06-gce-mcp-standalone)。
import process from 'node:process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const SCRIPTS = path.dirname(fileURLToPath(import.meta.url));
const SERVER = path.join(SCRIPTS, 'group-chat-mcp.mjs');
const BUDGET = 3200; // M3 四→六工具;评审实测六工具 wire ≈2881 chars

const live = process.argv.includes('--live');
const binIdx = process.argv.indexOf('--bin');
const binArg = binIdx !== -1 ? process.argv[binIdx + 1] : null;
// 拒绝吞掉下一个 flag(--bin --live 会把 "--live" 当 bin 路径 spawn)
if (binIdx !== -1 && (!binArg || binArg.startsWith('--'))) { console.error('FAIL: --bin 需要一个路径值'); process.exit(1); }
const binPath = binArg && (binArg === '~' ? os.homedir() : binArg.startsWith('~/') ? path.join(os.homedir(), binArg.slice(2)) : binArg);

const { Client } = await import('@modelcontextprotocol/sdk/client/index.js');
const { StdioClientTransport } = await import('@modelcontextprotocol/sdk/client/stdio.js');

const client = new Client({ name: 'group-chat-mcp-smoke', version: '1.0.0' });
const transport = binPath
  ? new StdioClientTransport({ command: binPath, args: [] })
  : new StdioClientTransport({ command: process.execPath, args: [SERVER] });
await client.connect(transport);
process.stderr.write(`[smoke] server spawned over stdio: ${binPath || `node ${SERVER}`}\n`);

const fail = (msg) => { console.error(`FAIL: ${msg}`); process.exitCode = 1; };

try {
  // 1) tools/list:六工具(M3 起)+ wire 预算(宿主注入 context 的地面真值)
  const { tools } = await client.listTools();
  const names = tools.map((t) => t.name);
  const want = ['start_discussion', 'discussion_status', 'discussion_result', 'cancel_discussion', 'interrupt_discussion', 'inject_message'];
  if (JSON.stringify(names) !== JSON.stringify(want)) fail(`tools/list 期望 ${want} 实得 ${names}`);
  const chars = JSON.stringify(tools.map(({ name, description, inputSchema }) => ({ name, description, inputSchema }))).length;
  process.stderr.write(`[smoke] tools/list ok(6);wire schema ${chars} chars ≈ ${Math.round(chars / 4)} tokens(预算 ${BUDGET})\n`);
  if (chars > BUDGET) fail(`wire 预算超支:${chars} > ${BUDGET}`);

  // 2) handler 链:不存在的 session → 可操作错误(daemon 两态皆算过)
  const probe = await client.callTool({ name: 'discussion_status', arguments: { session_id: 'smoke-nonexistent' } });
  const probeText = probe.content?.[0]?.text || '';
  const daemonUp = !/daemon/.test(JSON.parse(probeText).error);
  if (!probe.isError) fail('discussion_status(不存在 id)应走 isError');
  if (!/session 不存在|daemon/.test(JSON.parse(probeText).error)) fail(`错误文案不可操作:${probeText}`);
  process.stderr.write(`[smoke] handler 链 ok;daemon ${daemonUp ? '在跑' : '没跑(非 live 冒烟允许)'}\n`);

  if (!live) {
    console.log('SMOKE PASS (non-live):spawn + tools/list + 预算 + handler 错误链');
  } else {
    if (!daemonUp) { fail('daemon 没跑,--live 需要:scripts/daemon.sh start'); }
    else {
      // 3) 全链:小阵容真跑(arch = moderator + 2 人;5-15 分钟,数十万 token)
      const started = await client.callTool({
        name: 'start_discussion',
        arguments: {
          topic: '冒烟验收:请用一句话各自陈述「transcript 落盘位置应包含时间戳」的理由,主持人直接收官。',
          cwd: path.resolve(SCRIPTS, '..'), // 讨论 cwd = 仓库根(转录落 <cwd>/out/,与 M1 同处)
          preset: 'arch',
        },
      });
      if (started.isError) fail(`start 失败:${started.content?.[0]?.text}`);
      const { session_id } = JSON.parse(started.content[0].text);
      process.stderr.write(`[smoke] started ${session_id};轮询(arch 小阵容也要数分钟)…\n`);
      let last;
      for (let i = 0; i < 90; i++) {
        await sleep(10_000);
        const st = await client.callTool({ name: 'discussion_status', arguments: { session_id } });
        last = JSON.parse(st.content[0].text);
        process.stderr.write(`[smoke] poll #${i + 1}: busy=${last.busy} stop_reason=${last.stop_reason ?? '-'} elapsed=${last.elapsed_s ?? '-'}s\n`);
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
  }
} catch (e) {
  fail(e.stack || e.message);
} finally {
  await client.close().catch(() => {});
}
