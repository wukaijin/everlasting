#!/usr/bin/env node
// group-chat-mcp-deploy.mjs — MCP server standalone bin 部署器(GCE 部署面收口,任务 09-06-gce-mcp-standalone)。
//
// 把 scripts/group-chat-mcp-standalone-entry.mjs 用 bun --compile 打成单文件
// 可执行(内嵌运行时 + SDK,免 node / 免 node_modules / 免源码检出),装到
// XDG data 根,并把宿主 user-scope 配置原位切换到 bin。引擎与 mcp.mjs
// 一行不改——CLI 壳误判的根因与哨兵解法见任务 research/bun-compile-feasibility.md。
//
// 零 npm 依赖:node stdlib 自足,bun 只是被调用的外部工具(D1)。
//
// 用法:
//   node scripts/group-chat-mcp-deploy.mjs                # 默认:build → install → config
//   node scripts/group-chat-mcp-deploy.mjs --revert       # 配置回 node 挂载体(bin 保留;开发态默认)
//   node scripts/group-chat-mcp-deploy.mjs --uninstall    # 删配置项 + 删 bin
//   --config <path>                                       # 覆盖默认 ~/.zcode/cli/config.json(支持 ~ 展开)
//
// 语义约束:
// - 配置写前留单份回滚备份(config.json.mcp-deploy.bak,覆盖不堆积);读-改-写,
//   单用户 dev 语境接受宿主并发写窗口(PRD 约束)。
// - 全程幂等:重复跑 deploy/revert/uninstall 结果一致。
// - stdout 只放最终结论与下一步命令;过程诊断走 stderr。

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const SCRIPTS_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPTS_DIR, '..');
const ENTRY = path.join(SCRIPTS_DIR, 'group-chat-mcp-standalone-entry.mjs');

export const SERVER_NAME = 'everlasting-group-chat';
const BIN_NAME = 'everlasting-group-chat-mcp';

// ---------------------------------------------------------------------------
// 纯逻辑区(零副作用,供 node --test)
// ---------------------------------------------------------------------------

/** `~` 展开(--config 手写路径常带)。 */
export function expandHome(p) {
  if (p === '~') return os.homedir();
  if (p.startsWith('~/')) return path.join(os.homedir(), p.slice(2));
  return p;
}

function parseConfig(rawText) {
  const obj = JSON.parse(rawText);
  if (!obj || typeof obj !== 'object' || Array.isArray(obj)) throw new Error('配置顶层必须是 JSON object');
  return obj;
}

/** 定位 servers 容器:优先 `mcp.servers`(ZCode user-scope 实际形状,官方
 * 配置指南 + 本机 config.json 实证;任务文档把层级压扁成顶层 `servers` 是
 * research 记误),兼容顶层 `servers`(其他宿主形态)。两者皆无返回 null。 */
function serversContainer(obj) {
  const nested = obj.mcp?.servers;
  if (nested && typeof nested === 'object' && !Array.isArray(nested)) return nested;
  if (obj.servers && typeof obj.servers === 'object' && !Array.isArray(obj.servers)) return obj.servers;
  return null;
}

/** 读-改-写 servers 配置:保留其他顶层键与其他 server;目标 server 原位替换
 * 为 `{ command }` 形体(args 可选,revert 挂载体才有);空/缺失输入建骨架。
 * 返回序列化文本(2 空格缩进 + 结尾换行)。JSON 损坏时抛错——CLI 侧在
 * 备份/写入之前,绝不覆盖用户手工维护的配置。 */
export function mergeServersConfig(rawText, { serverName, command, args }) {
  if (!serverName) throw new Error('mergeServersConfig:需要 serverName');
  if (!command) throw new Error('mergeServersConfig:需要 command');
  let obj;
  if (rawText == null || rawText.trim() === '') {
    obj = { mcp: { servers: {} } }; // 骨架按 ZCode user-scope 形状(唯一在用宿主)
  } else {
    obj = parseConfig(rawText);
  }
  let servers = serversContainer(obj);
  if (!servers) {
    if (!obj.mcp || typeof obj.mcp !== 'object' || Array.isArray(obj.mcp)) obj.mcp = {};
    obj.mcp.servers = {};
    servers = obj.mcp.servers;
  }
  servers[serverName] = args ? { command, args } : { command };
  return JSON.stringify(obj, null, 2) + '\n';
}

/** 删单个 server 项(uninstall 配置面):返回 { text, removed };无容器/无此项
 * 时 text=null 表示「无需写」,幂等不触文件。 */
export function removeServerEntry(rawText, serverName) {
  if (!serverName) throw new Error('removeServerEntry:需要 serverName');
  if (rawText == null || rawText.trim() === '') return { text: null, removed: false };
  const obj = parseConfig(rawText);
  const servers = serversContainer(obj);
  if (!servers || !(serverName in servers)) return { text: null, removed: false };
  delete servers[serverName];
  return { text: JSON.stringify(obj, null, 2) + '\n', removed: true };
}

/** 读当前 server 项(部署摘要用;文件缺失/无项 → null)。 */
export function readServerEntry(rawText, serverName) {
  if (rawText == null || rawText.trim() === '') return null;
  return serversContainer(parseConfig(rawText))?.[serverName] ?? null;
}

/** revert 挂载体:node 直跑源码(开发态默认——改 .mjs 无需重编译,D6)。 */
export function buildRevertEntry(scriptsDir) {
  return { command: 'node', args: [path.join(scriptsDir, 'group-chat-mcp.mjs')] };
}

/** CLI 参数纯解析(--revert 与 --uninstall 互斥;--config 取下一个值)。 */
export function parseArgs(argv) {
  const out = { mode: 'deploy', configPath: null, help: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--revert' || a === '--uninstall') {
      const mode = a === '--revert' ? 'revert' : 'uninstall';
      if (out.mode !== 'deploy' && out.mode !== mode) throw new Error('--revert 与 --uninstall 互斥');
      out.mode = mode;
    } else if (a === '--config') {
      out.configPath = argv[++i];
      // 拒绝吞掉下一个 flag(--config --revert 会把 "--revert" 当路径,build 完写出一个同名垃圾文件)
      if (!out.configPath || out.configPath.startsWith('--')) throw new Error('--config 需要一个路径值');
    } else if (a === '--help' || a === '-h') {
      out.help = true;
    } else {
      throw new Error(`未知参数 "${a}"(用法见 --help)`);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// CLI 区(build / install / config / revert / uninstall)
// ---------------------------------------------------------------------------

/** bin 落点:XDG data 根与 daemon/DB 同根(D3)。 */
function binTarget() {
  const dataHome = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share');
  const bin = path.join(dataHome, 'dev.everlasting.app', 'bin', BIN_NAME);
  return { dir: path.dirname(bin), bin };
}

/** bun 定位:先 PATH,miss 再试官方默认安装位 ~/.bun/bin/bun;全 miss 报中文
 * 错误 + 安装提示(环境事实:bun 1.3.6 装在 ~/.bun,不在全部调用方 PATH)。 */
function resolveBun() {
  for (const candidate of ['bun', path.join(os.homedir(), '.bun', 'bin', 'bun')]) {
    try {
      execFileSync(candidate, ['--version'], { stdio: ['ignore', 'pipe', 'ignore'] });
      return candidate;
    } catch { /* ENOENT 或其他:试下一个候选 */ }
  }
  throw new Error('未找到 bun(bun --compile 是本部署器的构建工具链)。安装:curl -fsSL https://bun.sh/install | bash(https://bun.sh)');
}

function buildBin(bun) {
  const { dir, bin } = binTarget();
  fs.mkdirSync(dir, { recursive: true });
  const tmp = `${bin}.tmp-${process.pid}`; // 产物先落临时名再 rename:半成品绝不占据正式路径
  execFileSync(bun, ['build', '--compile', ENTRY, '--outfile', tmp], { stdio: 'inherit', cwd: REPO_ROOT });
  fs.chmodSync(tmp, 0o755);
  fs.renameSync(tmp, bin);
  return bin;
}

/** sidecar build-info:git short rev + ISO 时间戳(诊断 stale bin——bin 与
 * 源码演进脱节时对照一眼即知)。git 不可达写 n/a,不阻断部署。 */
function writeBuildInfo(bin) {
  let rev = 'n/a';
  try {
    // 任务文档记「git rev --short」是 shorthand;真命令是 rev-parse
    rev = execFileSync('git', ['rev-parse', '--short', 'HEAD'], { cwd: REPO_ROOT, encoding: 'utf8' }).trim() || 'n/a';
  } catch { /* 非 git 环境容忍 */ }
  const builtAt = new Date().toISOString();
  fs.writeFileSync(`${bin}.build-info`, `${rev}\n${builtAt}\n`);
  return { rev, builtAt };
}

/** 配置写盘通用体:读原文 → mergeFn 产新文 → 备份(单份覆盖)→ 写入。
 * mergeFn 返回 text=null 表示无需写(幂等不触文件);新文与现文逐字节一致
 * 时同样跳过——幂等重跑绝不 clobber 首跑备份(那份才是真回滚点)。 */
function applyConfig(configPath, mergeFn) {
  const exists = fs.existsSync(configPath);
  const raw = exists ? fs.readFileSync(configPath, 'utf8') : null;
  let applied;
  try {
    applied = mergeFn(raw);
  } catch (e) {
    throw new Error(`${configPath}:${e.message}`); // 报错带文件路径(损坏配置不指名文件不可操作)
  }
  if (applied.text == null || applied.text === raw) return { changed: false, raw };
  fs.mkdirSync(path.dirname(configPath), { recursive: true });
  if (exists) fs.copyFileSync(configPath, `${configPath}.mcp-deploy.bak`);
  fs.writeFileSync(configPath, applied.text);
  return { changed: true, raw };
}

function usage() {
  process.stdout.write([
    'group-chat-mcp-deploy.mjs — MCP server standalone bin 部署器(bun compile,零 npm 依赖)',
    '',
    '用法:',
    '  node scripts/group-chat-mcp-deploy.mjs                # 默认:build → install → config',
    '  node scripts/group-chat-mcp-deploy.mjs --revert       # 配置回 node 挂载体(bin 保留;开发态默认)',
    '  node scripts/group-chat-mcp-deploy.mjs --uninstall    # 删配置项 + 删 bin',
    '  --config <path>                                       # 覆盖默认 ~/.zcode/cli/config.json(支持 ~ 展开)',
    '',
  ].join('\n'));
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) { usage(); return 0; }
  const configPath = path.resolve(expandHome(opts.configPath || path.join(os.homedir(), '.zcode', 'cli', 'config.json')));

  if (opts.mode === 'deploy') {
    const bun = resolveBun();
    const bin = buildBin(bun);
    const { rev, builtAt } = writeBuildInfo(bin);
    const sizeMb = (fs.statSync(bin).size / 1048576).toFixed(1);
    const old = readServerEntry(fs.existsSync(configPath) ? fs.readFileSync(configPath, 'utf8') : null, SERVER_NAME);
    const { changed } = applyConfig(configPath, (raw) => ({ text: mergeServersConfig(raw, { serverName: SERVER_NAME, command: bin }) }));
    process.stderr.write(`[deploy] build: ${bun} --compile → ${bin}(${sizeMb} MB)\n`);
    process.stderr.write(`[deploy] build-info: rev ${rev} @ ${builtAt}\n`);
    process.stderr.write(`[deploy] config: ${configPath}${changed ? '' : '(已一致,未触盘)'}\n`);
    process.stderr.write(`[deploy]   旧: ${old ? JSON.stringify(old) : '(无挂载)'}\n`);
    process.stderr.write(`[deploy]   新: ${JSON.stringify({ command: bin })}${changed ? `(备份:${configPath}.mcp-deploy.bak)` : ''}\n`);
    console.log('DEPLOY OK:bin 已装,user-scope 配置已切到 standalone bin(重启宿主会话生效)');
    console.log(`验证:node scripts/group-chat-mcp-smoke.mjs --bin ${bin}`);
    console.log('回滚:node scripts/group-chat-mcp-deploy.mjs --revert(回 node 挂载)/ --uninstall(删配置项+删 bin)');
    return 0;
  }

  if (opts.mode === 'revert') {
    const entry = buildRevertEntry(SCRIPTS_DIR);
    const { changed } = applyConfig(configPath, (raw) => ({ text: mergeServersConfig(raw, { serverName: SERVER_NAME, command: entry.command, args: entry.args }) }));
    process.stderr.write(`[revert] config: ${configPath} → ${JSON.stringify(entry)}${changed ? `(备份:${configPath}.mcp-deploy.bak)` : '(本就是 node 挂载,未动)'}\n`);
    console.log(`REVERT OK:配置已${changed ? '切回' : '保持在'} node 挂载(开发态;bin 保留在 ${binTarget().bin})`);
    console.log('验证:node scripts/group-chat-mcp-smoke.mjs(node 直连冒烟)');
    return 0;
  }

  // uninstall:删配置项 + 删 bin(含 sidecar);各项独立幂等
  const { changed } = applyConfig(configPath, (raw) => removeServerEntry(raw, SERVER_NAME));
  const { bin } = binTarget();
  const binGone = fs.existsSync(bin);
  fs.rmSync(bin, { force: true });
  fs.rmSync(`${bin}.build-info`, { force: true });
  process.stderr.write(`[uninstall] config: ${SERVER_NAME} 项${changed ? `已删(备份:${configPath}.mcp-deploy.bak)` : '本就不存在'}\n`);
  process.stderr.write(`[uninstall] bin: ${bin}${binGone ? ' 已删' : ' 本就不存在'}\n`);
  console.log('UNINSTALL OK:配置项与 bin 已清(node 挂载如需恢复:重新跑 deploy 或手改配置)');
  return 0;
}

const isMain = (() => {
  try {
    return process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch { return false; }
})();

if (isMain) {
  main()
    .then((code) => process.exit(code ?? 0))
    .catch((e) => {
      process.stderr.write(`[deploy] 错误:${e.message}\n`);
      process.exit(1);
    });
}
