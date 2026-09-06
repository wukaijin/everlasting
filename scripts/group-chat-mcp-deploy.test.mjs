// group-chat-mcp-deploy 纯逻辑单测(node:test;vitest 不收 scripts/)。
// 跑法:node --test scripts/group-chat-mcp-deploy.test.mjs
// 只测纯函数区;build/install/config 落盘路径由真跑 deploy CLI + smoke --bin 验收。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';

import {
  SERVER_NAME, expandHome, mergeServersConfig, removeServerEntry,
  readServerEntry, buildRevertEntry, parseArgs,
} from './group-chat-mcp-deploy.mjs';

// 既有 config 样本:ZCode user-scope 实际形状(mcp.servers 嵌套)+ 其他顶层键 + 其他 server
const EXISTING = JSON.stringify({
  plugins: { enabledPlugins: { 'some-plugin@official': true } },
  mcp: {
    servers: {
      'other-server': { command: 'uvx', args: ['some-mcp'] },
      [SERVER_NAME]: { command: 'node', args: ['/old/checkout/scripts/group-chat-mcp.mjs'] },
    },
  },
});

test('merge:保留其他顶层键与其他 server;目标 server 原位替换为 { command } 无 args;缩进+尾换行', () => {
  const text = mergeServersConfig(EXISTING, { serverName: SERVER_NAME, command: '/data/bin/everlasting-group-chat-mcp' });
  const obj = JSON.parse(text);
  assert.deepEqual(obj.plugins, { enabledPlugins: { 'some-plugin@official': true } }, '其他顶层键原样');
  assert.deepEqual(obj.mcp.servers['other-server'], { command: 'uvx', args: ['some-mcp'] }, '其他 server 原样');
  assert.deepEqual(obj.mcp.servers[SERVER_NAME], { command: '/data/bin/everlasting-group-chat-mcp' }, '挂载体无 args');
  assert.ok(text.endsWith('\n') && !text.endsWith('\n\n'), '结尾单换行');
  assert.ok(text.includes('\n  "mcp"'), '2 空格缩进');
  assert.equal(text, JSON.stringify(obj, null, 2) + '\n');
});

test('merge:幂等(连跑两次结果逐字节相同)', () => {
  const once = mergeServersConfig(EXISTING, { serverName: SERVER_NAME, command: '/bin/mcp' });
  const twice = mergeServersConfig(once, { serverName: SERVER_NAME, command: '/bin/mcp' });
  assert.equal(twice, once);
  // 且从既有 node 挂载切到 bin 再切回同一路径,同样收敛
  assert.equal(mergeServersConfig(once, { serverName: SERVER_NAME, command: '/bin/mcp' }), once);
});

test('merge:缺文件/无容器建骨架(ZCode mcp.servers 形状);兼容顶层 servers 容器;JSON 损坏报错', () => {
  const fresh = JSON.parse(mergeServersConfig(null, { serverName: SERVER_NAME, command: '/bin/mcp' }));
  assert.deepEqual(fresh, { mcp: { servers: { [SERVER_NAME]: { command: '/bin/mcp' } } } });

  const blank = JSON.parse(mergeServersConfig('   ', { serverName: SERVER_NAME, command: '/bin/mcp' }));
  assert.deepEqual(blank.mcp.servers[SERVER_NAME], { command: '/bin/mcp' });

  // 顶层 servers(任务文档记法/其他宿主形态)→ 原容器内更新,不另建 mcp
  const legacy = JSON.stringify({ servers: { [SERVER_NAME]: { command: 'old' } } });
  const merged = JSON.parse(mergeServersConfig(legacy, { serverName: SERVER_NAME, command: '/bin/mcp' }));
  assert.deepEqual(merged, { servers: { [SERVER_NAME]: { command: '/bin/mcp' } } });

  assert.throws(() => mergeServersConfig('{broken', { serverName: SERVER_NAME, command: '/bin/mcp' }), /JSON/);
});

test('revert 挂载体与读删语义:buildRevertEntry 形状;removeServerEntry 幂等;readServerEntry 兜底 null', () => {
  assert.deepEqual(buildRevertEntry('/repo/scripts'), { command: 'node', args: ['/repo/scripts/group-chat-mcp.mjs'] });

  const removed = removeServerEntry(EXISTING, SERVER_NAME);
  assert.equal(removed.removed, true);
  const obj = JSON.parse(removed.text);
  assert.equal(SERVER_NAME in obj.mcp.servers, false);
  assert.ok('other-server' in obj.mcp.servers, '其他 server 保留');
  assert.equal(removeServerEntry(removed.text, SERVER_NAME).removed, false, '再删一次 = 无事发生(幂等)');
  assert.equal(removeServerEntry(removed.text, SERVER_NAME).text, null, '无此项不触文件');

  assert.equal(readServerEntry(EXISTING, SERVER_NAME).command, 'node');
  assert.equal(readServerEntry(removed.text, SERVER_NAME), null);
  assert.equal(readServerEntry(null, SERVER_NAME), null, '缺文件 → null 不抛');
});

test('parseArgs:默认 deploy;--revert/--uninstall 互斥;--config 取值;未知参数报错', () => {
  assert.deepEqual(parseArgs([]), { mode: 'deploy', configPath: null, help: false });
  assert.equal(parseArgs(['--revert']).mode, 'revert');
  assert.equal(parseArgs(['--uninstall']).mode, 'uninstall');
  assert.deepEqual(parseArgs(['--config', '~/custom.json', '--revert']), { mode: 'revert', configPath: '~/custom.json', help: false });
  assert.throws(() => parseArgs(['--revert', '--uninstall']), /互斥/);
  assert.throws(() => parseArgs(['--config']), /--config 需要一个路径值/);
  assert.throws(() => parseArgs(['--config', '--revert']), /--config 需要一个路径值/, 'flag 值不被吞当路径');
  assert.throws(() => parseArgs(['--wat']), /未知参数/);
  assert.equal(expandHome('~/x/y.json'), path.join(os.homedir(), 'x/y.json'));
  assert.equal(expandHome('/abs/x.json'), '/abs/x.json');
});
