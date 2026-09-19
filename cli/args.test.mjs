// args.test.mjs — 参数解析/横切值校验纯函数单测(node --test;不打真 daemon)。
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseCli,
  resolveBaseUrl,
  validateMode,
  defaultModeFor,
  parseTimeout,
  DEFAULT_TIMEOUT_S,
  DEFAULT_BASE,
  UsageError,
  COMMANDS,
} from './lib/args.mjs';

const usageErr = (fn) => {
  try {
    fn();
  } catch (e) {
    assert.ok(e instanceof UsageError, `expected UsageError, got ${e?.constructor?.name}`);
    assert.equal(e.exitCode, 64);
    return e;
  }
  assert.fail('expected UsageError to be thrown');
};

test('parseCli: 裸命令', () => {
  const p = parseCli(['status']);
  assert.equal(p.command, 'status');
  assert.deepEqual(p.positionals, []);
  assert.equal(p.flags.output, undefined);
  assert.equal(p.flags.timeout, DEFAULT_TIMEOUT_S);
});

test('parseCli: 全局 flag 提取(值型 + 布尔型,任意位置)', () => {
  const p = parseCli(['chat', 'hi', '--output', 'json', '--verbose']);
  assert.equal(p.command, 'chat');
  assert.deepEqual(p.positionals, ['hi']);
  assert.equal(p.flags.output, 'json');
  assert.equal(p.flags.verbose, true);
});

test('parseCli: flag 在命令之前也合法', () => {
  const p = parseCli(['--output', 'json', 'status']);
  assert.equal(p.command, 'status');
  assert.equal(p.flags.output, 'json');
});

test('parseCli: --key=value 形式', () => {
  const p = parseCli(['chat', '--output=json', '--session=abc', 'hello']);
  assert.equal(p.flags.output, 'json');
  assert.equal(p.flags.session, 'abc');
  assert.deepEqual(p.positionals, ['hello']);
});

test('parseCli: 多个位置参数(chat 消息由调用方 join)', () => {
  const p = parseCli(['chat', 'hello', 'world']);
  assert.deepEqual(p.positionals, ['hello', 'world']);
});

test('parseCli: -- 之后全部按位置参数(消息以 - 开头)', () => {
  const p = parseCli(['chat', '--', '--weird-not-a-flag']);
  assert.deepEqual(p.positionals, ['--weird-not-a-flag']);
});

test('parseCli: 未知 flag → UsageError 64', () => {
  const e = usageErr(() => parseCli(['status', '--nope']));
  assert.match(e.message, /--nope/);
});

test('parseCli: 短 flag -h 等价 --help(任意位置)', () => {
  assert.equal(parseCli(['-h']).flags.help, true);
  assert.equal(parseCli(['status', '-h']).flags.help, true);
  assert.equal(parseCli(['chat', 'hi', '-h']).flags.help, true);
});

test('parseCli: 未知短 flag → UsageError 64(不落成位置参数)', () => {
  usageErr(() => parseCli(['-x']));
  usageErr(() => parseCli(['status', '-x']));
});

test('parseCli: 未知命令 → UsageError 64', () => {
  const e = usageErr(() => parseCli(['nope']));
  assert.match(e.message, /未知命令/);
});

test('parseCli: value 型 flag 缺值(末尾)→ 64', () => {
  usageErr(() => parseCli(['chat', 'hi', '--output']));
});

test('parseCli: value 型 flag 缺值(下一个是已知 flag)→ 64', () => {
  usageErr(() => parseCli(['chat', '--mode', '--output', 'json', 'hi']));
});

test('parseCli: 布尔 flag 带 = 值 → 64', () => {
  usageErr(() => parseCli(['chat', '--quiet=true']));
});

test('parseCli: 子命令专属 flag 越用 → 64', () => {
  usageErr(() => parseCli(['status', '--mode', 'plan']));
  usageErr(() => parseCli(['sessions', '--provider', 'x']));
});

test('parseCli: chat 专属 flag 合法', () => {
  const p = parseCli(['chat', 'go', '--mode', 'plan', '--ephemeral', '--model', 'm1', '--project', '/x']);
  assert.equal(p.flags.mode, 'plan');
  assert.equal(p.flags.ephemeral, true);
  assert.equal(p.flags.model, 'm1');
  assert.equal(p.flags.project, '/x');
});

test('parseCli: --output 非法值 → 64', () => {
  usageErr(() => parseCli(['status', '--output', 'yaml']));
});

test('resolveBaseUrl: 默认 > env > flag 三趟', () => {
  assert.equal(resolveBaseUrl(undefined, undefined), DEFAULT_BASE);
  assert.equal(resolveBaseUrl(undefined, 'http://env:1'), 'http://env:1');
  assert.equal(resolveBaseUrl('http://flag:2', 'http://env:1'), 'http://flag:2');
  assert.equal(resolveBaseUrl('http://flag:2/', undefined), 'http://flag:2'); // 尾斜杠规整
});

test('parseTimeout: 默认 540;非法 → 64', () => {
  assert.equal(parseTimeout(undefined), DEFAULT_TIMEOUT_S);
  assert.equal(parseTimeout('30'), 30);
  assert.equal(parseTimeout(90), 90);
  usageErr(() => parseTimeout('0'));
  usageErr(() => parseTimeout('-5'));
  usageErr(() => parseTimeout('abc'));
  usageErr(() => parseTimeout('1.5'));
});

test('validateMode: plan|edit|yolo 合法;plna 等 → 64(daemon lenient,CLI 必拦)', () => {
  assert.equal(validateMode('plan'), 'plan');
  assert.equal(validateMode('edit'), 'edit');
  assert.equal(validateMode('YOLO'), 'yolo'); // 大小写容忍
  const e = usageErr(() => validateMode('plna'));
  assert.match(e.message, /plan\|edit\|yolo/);
  usageErr(() => validateMode(''));
  usageErr(() => validateMode(undefined));
});

test('defaultModeFor: TTY=edit,非 TTY=plan(fail-closed)', () => {
  assert.equal(defaultModeFor(true), 'edit');
  assert.equal(defaultModeFor(false), 'plan');
});

test('COMMANDS: 六命令在册', () => {
  assert.deepEqual(COMMANDS.sort(), ['chat', 'models', 'projects', 'sessions', 'status', 'usage']);
});
