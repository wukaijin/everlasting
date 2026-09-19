// lib/args.mjs — 手写参数解析(design §7:零依赖,不用 commander)。
//
// 纯函数(node --test 覆盖):parseCli / resolveBaseUrl / validateMode /
// defaultModeFor / parseTimeout。IO 不在这里发生。
//
// 解析规则:
// - 首个非 flag token = 子命令(必须在 COMMANDS 内,否则 UsageError/64)。
// - 全局 flag 任意位置可出现;子命令专属 flag 只在该子命令下合法(否则 64,
//   严出好过静默吞:`evl status --mode plan` 直接报错而不是被忽略)。
// - `--key value` / `--key=value` / 布尔裸 `--flag`;单字符短形仅限布尔
//   (当前唯一 `-h`);`--` 之后全按位置参数。
// - 未知 flag → UsageError(64);value 型 flag 的值缺失 → UsageError(64)。
import process from 'node:process';

export const EXIT_USAGE = 64;

/** chat 等终态默认超时(秒)。540 = 宿主 bash 上限 10min − 60s 余量
 * (design §4;不变量:宿主 bash 超时 ≥ CLI --timeout + 60s)。 */
export const DEFAULT_TIMEOUT_S = 540;

export const DEFAULT_BASE = 'http://127.0.0.1:7456';

export class UsageError extends Error {
  constructor(message) {
    super(message);
    this.name = 'UsageError';
    this.exitCode = EXIT_USAGE;
  }
}

// kind: 'value' 取值 | 'bool' 裸开关;dest 进 flags 对象。
export const GLOBAL_FLAG_SPEC = {
  'base-url': { kind: 'value', dest: 'baseUrl' },
  output: { kind: 'value', dest: 'output' },
  quiet: { kind: 'bool', dest: 'quiet' },
  verbose: { kind: 'bool', dest: 'verbose' },
  timeout: { kind: 'value', dest: 'timeout' },
  'no-color': { kind: 'bool', dest: 'noColor' },
  'non-interactive': { kind: 'bool', dest: 'nonInteractive' },
  help: { kind: 'bool', dest: 'help' },
  h: { kind: 'bool', dest: 'help' },
  version: { kind: 'bool', dest: 'version' },
};

export const COMMAND_FLAG_SPEC = {
  status: {},
  chat: {
    mode: { kind: 'value', dest: 'mode' },
    session: { kind: 'value', dest: 'session' },
    ephemeral: { kind: 'bool', dest: 'ephemeral' },
    model: { kind: 'value', dest: 'model' },
    project: { kind: 'value', dest: 'project' },
  },
  sessions: {
    project: { kind: 'value', dest: 'project' },
  },
  projects: {},
  models: {},
  usage: {
    provider: { kind: 'value', dest: 'provider' },
  },
};

export const COMMANDS = Object.keys(COMMAND_FLAG_SPEC);

/** 解析 argv → {command, positionals, flags}。任何不合法处抛 UsageError(64)。 */
export function parseCli(argv, spec = { global: GLOBAL_FLAG_SPEC, commands: COMMAND_FLAG_SPEC }) {
  const flags = {};
  const positionals = [];
  let command = null;
  let afterDoubleDash = false;

  const knownFlagSpec = (name) =>
    spec.global[name] ?? (command != null ? spec.commands[command]?.[name] : undefined);

  for (let i = 0; i < argv.length; i++) {
    const tok = argv[i];

    if (afterDoubleDash) {
      pushPositional(tok);
      continue;
    }
    if (tok === '--') {
      afterDoubleDash = true;
      continue;
    }

    // 短 flag:仅支持单字符布尔型(当前唯一:-h)。值型一律用长形。
    if (tok.length === 2 && tok.charCodeAt(0) === 45 /* '-' */ && /[a-z]/i.test(tok[1])) {
      const entry = knownFlagSpec(tok[1]);
      if (!entry) {
        throw new UsageError(`未知 flag: ${tok}(见 evl --help)`);
      }
      if (entry.kind !== 'bool') {
        throw new UsageError(`${tok} 不支持短形(用 --${tok[1]} ...)`);
      }
      flags[entry.dest] = true;
      continue;
    }

    if (tok.startsWith('--') && tok.length > 2) {
      const eq = tok.indexOf('=');
      const name = eq === -1 ? tok.slice(2) : tok.slice(2, eq);
      const inlineValue = eq === -1 ? undefined : tok.slice(eq + 1);
      const entry = knownFlagSpec(name);
      if (!entry) {
        throw new UsageError(
          `未知 flag: --${name}${command ? `(命令 ${command})` : ''}(见 evl --help)`
        );
      }
      if (entry.kind === 'bool') {
        if (inlineValue !== undefined) {
          throw new UsageError(`--${name} 是布尔 flag,不接受值(--${name}=... 形式非法)`);
        }
        flags[entry.dest] = true;
        continue;
      }
      // value 型
      let value = inlineValue;
      if (value === undefined) {
        if (i + 1 >= argv.length) {
          throw new UsageError(`--${name} 缺少值`);
        }
        const next = argv[i + 1];
        // 下一个 token 是已知 flag 名 → 大概率漏写值(--mode --output json),拦下
        const nextName = next.startsWith('--') && next.length > 2 ? next.slice(2).split('=')[0] : null;
        if (nextName != null && knownFlagSpec(nextName)) {
          throw new UsageError(`--${name} 缺少值(下一个 token 是 flag:${next})`);
        }
        value = next;
        i++;
      }
      flags[entry.dest] = value;
      continue;
    }

    pushPositional(tok);

    function pushPositional(t) {
      if (command == null) {
        if (!spec.commands[t]) {
          throw new UsageError(`未知命令: ${t}(可用:${Object.keys(spec.commands).join(' | ')};见 evl --help)`);
        }
        command = t;
      } else {
        positionals.push(t);
      }
    }
  }

  // 横切值校验(解析期一并锁,使用方不再重复)
  if (flags.output !== undefined && flags.output !== 'text' && flags.output !== 'json') {
    throw new UsageError(`--output 非法值:${flags.output}(合法:text | json)`);
  }
  flags.timeout = parseTimeout(flags.timeout);

  return { command, positionals, flags };
}

/** base-url 三趟:flag > env > 默认;尾部斜杠规整。 */
export function resolveBaseUrl(flagValue, envValue) {
  const raw = flagValue || envValue || DEFAULT_BASE;
  return raw.replace(/\/+$/, '');
}

/** timeout 解析:undefined → 默认 540;非正整数 → UsageError。 */
export function parseTimeout(raw) {
  if (raw === undefined) return DEFAULT_TIMEOUT_S;
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0) {
    throw new UsageError(`--timeout 非法值:${raw}(需要正整数秒,默认 ${DEFAULT_TIMEOUT_S})`);
  }
  return n;
}

export const MODES = ['plan', 'edit', 'yolo'];

/** --mode 值域 CLI 侧校验(design §5):daemon 解析 lenient(未知值静默回退
 * edit = fail-open),所以必须在 CLI 拦;非法 → UsageError(64)。 */
export function validateMode(raw) {
  const v = String(raw ?? '').trim().toLowerCase();
  if (!MODES.includes(v)) {
    throw new UsageError(
      `--mode 非法值: ${raw}(合法:${MODES.join('|')};daemon 侧未知值会静默回退 edit,故 CLI 拦截)`
    );
  }
  return v;
}

/** 未显式给 --mode 时的默认双模(design §5):TTY=edit(session 默认,不动),
 * 非 TTY=plan(fail-closed——默认值即安全边界)。 */
export function defaultModeFor(interactive) {
  return interactive ? 'edit' : 'plan';
}
