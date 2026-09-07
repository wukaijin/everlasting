#!/usr/bin/env node
// group-chat-run — GCE-M1 群聊审议驱动入口(engine;skill/未来 MCP 是门面)。
//
// 平台三层:daemon 群聊原语(零改动)/ 本脚本(确定性引擎)/
// .agents/skills/group-chat/(LLM 指引,零逻辑)。生命周期语义
// (busy/stop_reason 轮询、中断 cancel、转录落盘)全部锁死在这份
// 实现里 —— GC1-G7 的教训:编排消费逻辑绝不能散进 prompt。
//
// 用法速查(完整 --help 见 bottom):
//   node scripts/group-chat-run.mjs projects          # 建群信息①:项目目录
//   node scripts/group-chat-run.mjs models            # 建群信息②:模型目录
//   node scripts/group-chat-run.mjs presets           # 建群信息③:预设配方
//   node scripts/group-chat-run.mjs run --preset review --topic "..." [--dry-run]
//
// 语义约束(设计定案,勿改):
// - 全程不挂 SSE 连接 —— 保持 GC3 无观察者 8s 快拒;进度靠轮询
//   list_sessions 的 busy/stop_reason(GC1/GC2 编排级粒度,轮间不回落)。
// - 中断(超时/SIGINT)→ cancel_chat(request_id)停编排保 session;
//   部分转录照常落盘(post-mortem 依据)。--cleanup 仅成功路径删 session。
// - 转录默认落 everlasting 仓库根的 out/(按本脚本位置推导,非 CWD)
//   —— 嵌套消费(daemon 单聊后台 shell 调用)时外层 cwd 是别的项目。
// - daemon 不可达的错误文案必须带 OS 错误翻译(EPERM → "Operation not
//   permitted"):沙箱升级分类器 sandbox::classify_block 按该字面串触发,
//   嵌套消费时外层 shell 靠它脱沙箱重跑(2026-09-06 live 实证)。

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

// GCE-M4a(R7 preset 单一事实源):预设定义抽到本目录的
// group-chat-presets.json(引擎 / 前端创建表单 / 未来消费方读同一份)。
// 用 **静态 import** 而非 fs.readFileSync:bun --compile 的 standalone
// bin 只内嵌模块图,readFileSync 的旁路文件不会被打包(deploy 面
// group-chat-mcp-deploy.mjs 依赖此行为)。JSON 里 persona 按 kind 引用
// (arch/product/backend/outsider)+ persona_common 单点存放公共纪律,
// compose 出的 persona_md 与旧内置常量逐字节同形(单测锁)。
import presetsFile from './group-chat-presets.json' with { type: 'json' };

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = path.resolve(path.dirname(SCRIPT_PATH), '..');
export const DEFAULT_BASE = process.env.EVERLASTING_BASE || 'http://127.0.0.1:7456';
const DEFAULT_TIMEOUT_S = 30 * 60; // 30min:两场 live 实测 9-16min,留余量
const POLL_INTERVAL_S = 10;
const CANCEL_SETTLE_S = 60; // cancel 后等 stop_reason=cancelled 落地的兜底窗

// 退出码契约(design.md):stop_reason 各值一档,1 留给脚本自身错误。
// GCE P1a(09-06-gc-p1a-checkpoint-resume):补 interrupted 档 —— daemon
// 进程级中断(boot sweep 标记,checkpoint 行残留)。它是可续跑态而非
// 脚本错误:落在 5(区别于 1),转录 note 提示 resume 出口。
export const EXIT = {
  scriptError: 1,
  groupChatEnd: 0,
  maxRounds: 2,
  cancelled: 3,
  error: 4,
  interrupted: 5,
};
const EXIT_BY_STOP_REASON = {
  group_chat_end: EXIT.groupChatEnd,
  max_rounds: EXIT.maxRounds,
  cancelled: EXIT.cancelled,
  error: EXIT.error,
  interrupted: EXIT.interrupted,
};

// ---------------------------------------------------------------------------
// 预设(R7 单一事实源:group-chat-presets.json)。review = 两场 live
// (09-05/06)验证过的阵容;模型引用用**名字**(run 时经
// normalizeModelRef 解析成 UUID —— daemon catalog 只认 UUID,
// DAEMON-API.md「模型引用只认 UUID(名字由脚本解析)」;前端展开时
// 同样做名字→UUID 解析)。persona 只写「视角边界」—— 公共发言纪律
// 单点存放在 JSON 的 persona_common(评审团 2026-09-06 verdict:三份
// persona 重复同一段纪律,内容资产需收敛)。persona 内容与议题无关:
// 议题经 --topic 传入,不要在 persona 里引用具体项目文档(上版引
// BUGLIST 是反面教材,live 抓出)。模型失配时报错并给出 models 内省
// 提示,绝不静默降级。
// ---------------------------------------------------------------------------

/** JSON → 运行时 PRESETS 形状(与旧内置常量同构):participants 的
 * persona kind 展开为完整 persona_md(边界 + "\n\n" + 公共纪律)。
 * 导出供单测锁「JSON 是唯一事实源 + 组装确定性」。 */
export function composePresets(file) {
  if (!file || typeof file !== 'object') throw new Error('group-chat-presets.json: 顶层必须是 object');
  const common = String(file.persona_common || '');
  const persona = (kind) => {
    const base = file.personas?.[kind];
    if (!base) throw new Error(`group-chat-presets.json: 缺 persona "${kind}"`);
    return `${base}\n\n${common}`;
  };
  const entries = Object.entries(file.presets || {});
  if (!entries.length) throw new Error('group-chat-presets.json: presets 不能为空');
  const out = {};
  for (const [name, preset] of entries) {
    if (!preset.moderator_model || !Array.isArray(preset.participants) || !preset.participants.length) {
      throw new Error(`group-chat-presets.json: 预设 "${name}" 缺 moderator_model 或 participants`);
    }
    out[name] = {
      description: preset.description,
      moderator_model: preset.moderator_model,
      participants: preset.participants.map((p) => ({ name: p.name, model: p.model, persona_md: persona(p.persona) })),
    };
  }
  return out;
}

export const PRESETS = composePresets(presetsFile);

// ---------------------------------------------------------------------------
// 纯函数区(M2 MCP 工具将直接 import 本区;CLI 与未来 MCP 都只是薄壳)
// 单测:scripts/group-chat-run.test.mjs(node:test,shape 断言非快照)
// ---------------------------------------------------------------------------

/**
 * 解析参与者配置。覆盖面(评审团 verdict 砍掉了 --add/--drop 两个
 * 高错误率旗标):`--participants` 整名单替换(增删语义的超集)+
 * `--set` 单人 model/persona 两级。预设名缺失 / 名单为空 / 重名 /
 * --set 目标不存在 → 明确报错。
 */
export function resolveParticipants({ preset, participantsJson, set }) {
  let list;
  if (participantsJson) {
    list = JSON.parse(participantsJson);
    if (!Array.isArray(list) || list.length === 0) throw new Error('--participants 必须是非空 JSON 数组 [{name, model, persona_md?}]');
  } else {
    if (!PRESETS[preset]) {
      throw new Error(`未知预设 "${preset}";可用:${Object.keys(PRESETS).join(' / ')}(查看细节:presets 子命令)`);
    }
    list = PRESETS[preset].participants.map((p) => ({ ...p }));
  }
  for (const p of list) {
    if (!p.name || !p.model) throw new Error(`参与者缺 name/model:${JSON.stringify(p)}`);
  }
  const names = new Set();
  for (const p of list) {
    if (names.has(p.name)) throw new Error(`参与者重名:"${p.name}"`);
    names.add(p.name);
  }
  // --set name.model=<id> / --set name.persona=@file|文本(可重复)
  for (const one of set || []) {
    const m = one.match(/^(\S+)\.(model|persona)=(.+)$/);
    if (!m) throw new Error(`--set 格式应为 name.model=<id> 或 name.persona=@file|文本,收到 "${one}"`);
    const [, targetName, field, value] = m;
    const target = list.find((p) => p.name === targetName);
    if (!target) throw new Error(`--set 目标参与者 "${targetName}" 不在名单:${[...names].join(' / ')}`);
    if (field === 'model') target.model = value;
    else target.persona_md = value.startsWith('@') ? fs.readFileSync(value.slice(1), 'utf8') : value;
  }
  return list;
}

/**
 * create_session 请求体。model 必须是 **model UUID**(catalog key,
 * state.rs:42;daemon 版 create_session 无 model_id 参数,UUID 走
 * session.model → moderator 解析的 fallback 路径命中 catalog)。
 */
export function buildCreateSessionBody({ projectId, projectPath, moderatorModel, participants, createdVia }) {
  return {
    project_id: projectId,
    initial_cwd: projectPath,
    model: moderatorModel,
    session_type: 'group_chat',
    // createdVia:召集通道归因(GCE-M2 D5):'script'(M1 CLI)/'mcp'(MCP server);
    // 缺失 = GUI/历史 session。增量键,daemon/GUI 不感知。
    metadata: createdVia ? { participants, created_via: createdVia } : { participants },
  };
}

/** agent/chat 首条 wire(daemon 版 fire-and-forget,编排后台跑)。 */
export function buildChatBody({ requestId, sessionId, topic }) {
  return {
    request_id: requestId,
    session_id: sessionId,
    messages: [{ role: 'user', content: topic }],
  };
}

// --- GCE-M3 inject_message 判定(mcp.mjs coreInject 消费;纯函数区) ---

/** inject 前置 busy guard(评审 P1-1 主防护):空闲/已收官群聊 session 一旦
 * fireChat 会重启编排器并无条件 clear_group_chat_lifecycle(抹上一场
 * stop_reason + summary,cancel 也救不回),故非 busy 一律不发起。
 * busy 只信 === true(session-busy-visibility 双源合流同规,additive wire)。 */
export function injectGuardDecision(summary) {
  if (summary?.busy === true) return { allowed: true };
  return {
    allowed: false,
    reason: `目标不是进行中的群聊讨论(busy=${summary?.busy ?? 'unknown'}, stop_reason=${summary?.stop_reason ?? 'null'})。注入只对进行中的讨论有效;发起新讨论请用 start_discussion。`,
  };
}

/** inject 受理判定:agent/chat 恒 JSON 返回 ChatAcceptance(serde tag)。
 * wire:`{"status":"injected"}`(注入成功)/ `{"status":"started"}`
 * (打在空闲群聊 = 误起新讨论)/ `{"status":"queued",id,position}`
 * (打在 busy 经典会话 = 误入 F1 队列)。后两者 misfire:调用方应用
 * 自有 rid 即时 cancelChat 止损(竞态兜底),再报语义错误。 */
export function interpretAcceptance(acceptance) {
  if (acceptance?.status === 'injected') return { kind: 'injected' };
  return { kind: 'misfire', status: acceptance?.status ?? 'unknown', cancelOwnRequest: true };
}

/** 物理路径比较(防 /repo/foo vs /repo/foobar 前缀陷阱,spec: project-cwd-boundary)。 */
function samePhysicalPath(a, b) {
  const norm = (p) => path.resolve(p).replace(/\/+$/, '');
  return norm(a) === norm(b);
}

/**
 * 名字/UUID 统一解析成 model UUID。daemon catalog 的 key 是 models.id
 * (state.rs:42),participants/moderator 引用无名字 fallback —— 名字进
 * metadata 会 participant_unresolved 跳轮。UUID 重装会变,故 preset 存
 * 可读名、run 时经此函数解析。
 */
export function normalizeModelRef(models, ref) {
  if (!ref) throw new Error('空模型引用');
  const byId = models.find((m) => m.id === ref);
  if (byId) return byId.id;
  // 两趟:先精确,后大小写不敏感。catalog 里存在「glm-5.3 的 modelName
  // == GLM-5.3-Flash 的 displayName(仅大小写差)」的真实撞车,单趟
  // toLowerCase 会让 'GLM-5.3' 命中谁取决于数组顺序——不可预期。
  const exact = models.find((m) => m.modelName === ref || m.displayName === ref);
  if (exact) return exact.id;
  const lower = String(ref).toLowerCase();
  const ci = models.find((m) => (m.modelName || '').toLowerCase() === lower || (m.displayName || '').toLowerCase() === lower);
  if (ci) return ci.id;
  const names = models.map((m) => m.modelName || m.displayName).join(' / ');
  throw new Error(`模型 "${ref}" 不在目录(现有:${names});models 子命令查清单`);
}

/** 校验并归一:名单与 moderator 的模型引用(名字或 UUID)→ 全部解析成 UUID。 */
export function validateModelRefs(models, { moderatorModel, participants }) {
  return {
    moderatorModelId: normalizeModelRef(models, moderatorModel),
    participants: participants.map((p) => ({ ...p, model: normalizeModelRef(models, p.model) })),
  };
}

/**
 * 工具轮证据链(评审团 verdict「转录四修」之二):从 wire content blocks
 * 提取 tool_use 的 name + 1 个关键参数,拼一行可读摘要;没有 tool_use
 * blocks(纯 tool_result 轮)返回 null。
 */
export function summarizeToolUses(content) {
  if (!Array.isArray(content)) return null;
  const uses = content.filter((b) => b && b.type === 'tool_use');
  if (!uses.length) return null;
  return uses.map((u) => {
    const inp = u.input || {};
    const key = inp.command || inp.file_path || inp.path || inp.pattern || inp.query || inp.topic || '';
    return `${u.name}{${String(key).slice(0, 60).replace(/\s+/g, ' ')}}`;
  }).join(' ');
}

/** 转录落点:everlasting 仓库根 out/(非 CWD —— 嵌套消费约束)。 */
export function defaultTranscriptPath(topic, rootDir = REPO_ROOT) {
  const slugBase = String(topic || 'discussion')
    .slice(0, 40)
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '')
    .toLowerCase() || 'discussion';
  const ts = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 14);
  // rootDir:MCP 消费时传讨论 cwd(转录留在证据基地,design §6 分叉声明);
  // M1 CLI 不传,默认引擎仓库根(本文件 :20 语义约束,勿改默认)。
  return path.join(rootDir, 'out', `group-chat-${slugBase}-${ts}.md`);
}

/**
 * 转录渲染(评审团 verdict「四修」采纳三项:blockquote 隔离碎格式 /
 * 工具轮证据链 / summary 缺失警告落文件;per-speaker token 延后 ——
 * turn_trace 行按 LLM 调用段落落库,与 speaker 对齐有歧义,硬 join
 * 会产错数,需群聊内部先在 trace 行打 speaker 标签,follow-up)。
 * speaker:group chat 写入 messages.speaker(moderator/参与者名);
 * null 时按既有惯例:工具轮记 `用户`,user 文本记 **用户**。
 */
export function renderTranscript({ session, messages, startedAtMs, stoppedAtMs, note, modelNames = {} }) {
  const p = (label, v) => (v === undefined || v === null || v === '' ? '' : `- ${label}: ${v}\n`);
  const durS = Math.round((stoppedAtMs - startedAtMs) / 1000);
  const disp = (id) => modelNames[id] || id;
  let roster = '主持人未知';
  try {
    const meta = typeof session.metadata === 'string' ? JSON.parse(session.metadata) : session.metadata;
    const parts = meta?.participants?.map((x) => `${x.name}/${disp(x.model)}`) || [];
    if (parts.length) roster = `${disp(session.model || session.model_id)} 主持 + ${parts.join(' + ')}`;
  } catch { /* metadata 坏了不影响转录 */ }
  const lines = [];
  lines.push(`# 群聊 ${session.title || '(untitled)'}(${new Date(startedAtMs).toISOString().slice(0, 10)})\n\n`);
  lines.push(p('session', `\`${session.id}\`(${roster})`));
  lines.push(p('结果', `${messages.length} 条消息 / ${durS}s / stop_reason=${session.stop_reason}${note ? ` / ${note}` : ''}`));
  const tk = session.input_tokens_total != null || session.output_tokens_total != null
    ? `in ${session.input_tokens_total ?? '?'} / out ${session.output_tokens_total ?? '?'}`
    : null;
  lines.push(p('token', tk));
  if (session.discussion_summary) {
    lines.push(`\n## discussion_summary\n\n${session.discussion_summary}\n`);
  } else if (session.stop_reason === 'group_chat_end') {
    lines.push('\n> ⚠️ 正常收官但 discussion_summary 缺失(moderator 未走 end_discussion;读转录尾段人工收束)\n');
  }
  lines.push('\n---\n');
  for (const m of messages) {
    const speaker = m.speaker ?? '用户';
    const isToolTurn = !m.speaker && (m.has_tool_calls || m.has_tool_results);
    if (isToolTurn) {
      const tools = summarizeToolUses(m.content);
      lines.push(`- seq${m.seq} \`${speaker}\`: ${tools ? `(工具调用 ${tools})` : '(工具调用轮/tool_result)'}\n`);
      continue;
    }
    const body = String(m.text || '').trim();
    if (!body) { lines.push(`- seq${m.seq} **${speaker}**: (空)\n`); continue; }
    // blockquote 隔离:LLM 输出里的列表/标题碎片不再打断转录的 seq 列表
    const quoted = body.split('\n').map((l) => `  > ${l}`).join('\n');
    lines.push(`- seq${m.seq} **${speaker}**:\n${quoted}\n`);
  }
  return lines.join('');
}

// ---------------------------------------------------------------------------
// API client(全 POST snake_case;唯一 GET:health)
// ---------------------------------------------------------------------------

// errno → 沙箱分类器认的字面串(sandbox/mod.rs classify_block:
// "Permission denied" / "Read-only file system" / "Operation not
// permitted")。缺这层翻译,嵌套消费里外层 shell 的升级链永远不触发。
export function fetchFailDetail(e) {
  const cause = e?.cause;
  if (!cause) return e?.message || String(e);
  const code = cause.code || '';
  const table = { EPERM: 'Operation not permitted (EPERM)', EACCES: 'Permission denied (EACCES)' };
  return `${cause.message || ''}${code ? ` [${table[code] || code}]` : ''}`;
}

async function api(base, route, { method = 'POST', body, okCodes = [200] } = {}) {
  const url = `${base}/api/v1/${route}`;
  const res = await fetch(url, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: method === 'GET' ? undefined : JSON.stringify(body ?? {}),
  }).catch((e) => {
    throw new Error(`daemon 不可达(${url}):${fetchFailDetail(e)};先确认 daemon 在跑(scripts/daemon.sh)`);
  });
  if (!okCodes.includes(res.status)) {
    const text = (await res.text()).slice(0, 300);
    throw new Error(`${method} /api/v1/${route} → HTTP ${res.status}: ${text}`);
  }
  return res.status === 204 ? null : res.json();
}

async function checkDaemon(base) {
  await api(base, 'health', { method: 'GET' });
}

/** 按路径解析 project;miss 则 create(turn-smoke.sh:119 先例)。
 * 列表必须带 `filter:{hidden:true}` 查全量——hidden(用户 GUI 侧栏隐藏)
 * 项目不在默认列表里,但 create_project 的唯一性检查查全表:过滤列表
 * match miss → create 撞 400 "already exists"(2026-09-06 vite-react-ts
 * live 实证)。 */
export async function resolveProject(base, projectPath) {
  const list = await api(base, 'projects/list_projects', { body: { filter: { hidden: true } } });
  const hit = list.find((proj) => samePhysicalPath(proj.path, projectPath));
  if (hit) return { id: hit.id, created: false };
  const created = await api(base, 'projects/create_project', { body: { path: projectPath } });
  return { id: created.id, created: true };
}

export function listModels(base) {
  return api(base, 'providers/list_models');
}

export function createSession(base, body) {
  return api(base, 'sessions/create_session', { body });
}

export function fireChat(base, body) {
  return api(base, 'agent/chat', { body });
}

export function pollSession(base, projectId, sessionId) {
  return api(base, 'sessions/list_sessions', { body: { project_id: projectId } })
    .then((list) => list.find((s) => s.id === sessionId) || null);
}

export function loadSession(base, sessionId) {
  return api(base, 'sessions/load_session', { body: { session_id: sessionId } });
}

export function cancelChat(base, requestId) {
  return api(base, 'cancel/cancel_chat', { body: { request_id: requestId } });
}

/** GCE-M3:体面打断(session 域收束——等在途发言完 → moderator 收束轮 →
 * stop_reason=preempted;与 cancelChat 的 rid 域硬停相对,DAEMON-API §4)。 */
export function preemptGroupChat(base, sessionId) {
  return api(base, 'cancel/preempt_group_chat', { body: { session_id: sessionId } });
}

export function deleteSession(base, sessionId) {
  return api(base, 'sessions/delete_session', { body: { session_id: sessionId } });
}

// ---------------------------------------------------------------------------
// run 主流程
// ---------------------------------------------------------------------------

function fmtElapsed(ms) {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s`;
}

async function run(argv) {
  const opt = {
    base: DEFAULT_BASE,
    project: process.cwd(),
    topic: undefined, topicFile: undefined,
    preset: 'review',
    participants: undefined,
    moderatorModel: undefined,
    set: [],
    timeout: DEFAULT_TIMEOUT_S,
    quiet: false,
    dryRun: false,
    out: undefined,
    cleanup: false,
  };
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      if (i + 1 >= argv.length) throw new Error(`参数 ${a} 缺值`);
      return argv[++i];
    };
    switch (a) {
      case '--base': opt.base = next(); break;
      case '--project': opt.project = next(); break;
      case '--topic': opt.topic = next(); break;
      case '--topic-file': opt.topicFile = next(); break;
      case '--preset': opt.preset = next(); break;
      case '--participants': opt.participants = next(); break;
      case '--moderator-model': opt.moderatorModel = next(); break;
      case '--set': opt.set.push(next()); break;
      case '--timeout': opt.timeout = Number(next()); break;
      case '--out': opt.out = next(); break;
      case '--quiet': opt.quiet = true; break;
      case '--dry-run': opt.dryRun = true; break;
      case '--cleanup': opt.cleanup = true; break;
      case '--help': case '-h': return printRunHelp();
      default: rest.push(a);
    }
  }
  if (rest.length) throw new Error(`未知参数:${rest.join(' ')}`);
  if (opt.topicFile) opt.topic = fs.readFileSync(opt.topicFile, 'utf8');
  if (!opt.topic || !opt.topic.trim()) throw new Error('缺议题:--topic <text> 或 --topic-file <path>(议题质量直接决定产出质量,见 skill 指引)');
  opt.project = path.resolve(opt.project);

  const participants = resolveParticipants({
    preset: opt.preset,
    participantsJson: opt.participants,
    set: opt.set,
  });
  const moderatorModel = opt.moderatorModel || PRESETS[opt.preset]?.moderator_model;
  if (!moderatorModel) throw new Error('缺主持人模型:--moderator-model <id>(models 子命令查目录)');

  const say = (...a) => { if (!opt.quiet) process.stderr.write(`${a.join(' ')}\n`); };
  // 关键行(session id / 转录路径 / 终态)不受 --quiet 影响 —— cron 静默进度但必须拿到路径
  const emit = (...a) => process.stderr.write(`${a.join(' ')}\n`);

  // --dry-run:纯静态模板,零网络(评审团 verdict:双态简化)。参数
  // 组装逻辑的回归保护在单测,不在 dry-run。
  if (opt.dryRun) {
    const createBody = buildCreateSessionBody({
      projectId: '<resolved-by-daemon>',
      projectPath: opt.project,
      moderatorModel: '<model-uuid>', // 真跑时由 normalizeModelRef 解析
      participants,
    });
    process.stdout.write('=== dry-run:将发出的请求(纯静态模板,不连 daemon)===\n');
    process.stdout.write(`POST /api/v1/sessions/create_session\n${JSON.stringify(createBody, null, 2)}\n\n`);
    process.stdout.write(`POST /api/v1/agent/chat\n${JSON.stringify(buildChatBody({ requestId: '<rid>', sessionId: '<sid>', topic: opt.topic }), null, 2)}\n`);
    process.stdout.write(`(转录落点:${opt.out || defaultTranscriptPath(opt.topic)};模型/项目解析发生在真跑时)\n`);
    return EXIT.groupChatEnd;
  }

  await checkDaemon(opt.base);
  const models = await listModels(opt.base);
  const norm = validateModelRefs(models, { moderatorModel, participants });
  const proj = await resolveProject(opt.base, opt.project);
  if (proj.created) say(`# project 不在列表,已创建:${opt.project}`);
  const createBody = buildCreateSessionBody({ projectId: proj.id, projectPath: opt.project, moderatorModel: norm.moderatorModelId, participants: norm.participants, createdVia: 'script' });
  const session = await createSession(opt.base, createBody);
  const sessionId = session.id;
  emit(`# session: ${sessionId}  moderator: ${moderatorModel}  participants: ${participants.map((p) => `${p.name}/${p.model}`).join(' + ')}`);

  const requestId = `group-chat-run-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  await fireChat(opt.base, buildChatBody({ requestId, sessionId, topic: opt.topic }));

  // 轮询终止语义(GC1/GC2):busy=true 进行中;busy=false+stop_reason≠null 终态。
  // 开跑竞态窗:chat fire-and-forget 后 busy 可能尚未置位,先等它出现。
  // 失败路径同样落转录(评审团 verdict:脚本错误不丢现场)—— pollLoop
  // 抛错时 catch 里导出再 rethrow,stop_reason 不篡改。
  const startedAtMs = Date.now();
  let sawBusy = false;
  let stopReason = null;
  let interrupted = false;
  let lastProgress = '';
  let scriptError = null;

  const onSigint = () => {
    if (interrupted) { // 第二次 Ctrl-C:不再等落定,直接导出现场退出
      scriptError = new Error('二次中断:放弃等待 cancelled 落定,导出现场退出');
      return;
    }
    interrupted = true;
    say('\n# SIGINT:cancel_chat 停编排(session 保留)…');
    cancelChat(opt.base, requestId).catch((e) => say(`# cancel 失败(可能已结束):${e.message}`));
  };
  process.on('SIGINT', onSigint);

  const pollLoop = async () => {
    while (true) {
      const s = await pollSession(opt.base, proj.id, sessionId);
      if (!s) throw new Error(`session 从列表消失:${sessionId}(被并发删除?)`);
      if (s.busy) sawBusy = true;
      if (s.stop_reason) return s.stop_reason;
      if (!sawBusy) {
        const graceMs = Date.now() - startedAtMs;
        if (graceMs > CANCEL_SETTLE_S * 1000) throw new Error(`chat 已发出但 ${CANCEL_SETTLE_S}s 内 busy 未置位(request_id=${requestId});查 daemon 日志:scripts/daemon.sh logs`);
      } else if (!opt.quiet) {
        const elapsed = fmtElapsed(Date.now() - startedAtMs);
        const line = `[${elapsed}] 讨论进行中…`;
        if (line !== lastProgress) { process.stderr.write(`\r${line}`); lastProgress = line; }
      }
      if (scriptError) throw scriptError;
      if (interrupted && Date.now() - startedAtMs > (opt.timeout + CANCEL_SETTLE_S) * 1000) return null;
      if (!interrupted && Date.now() - startedAtMs > opt.timeout * 1000) {
        say(`\n# 超时(${fmtElapsed(opt.timeout * 1000)}):cancel_chat 停编排(session 保留);调 --timeout 可延长`);
        interrupted = true;
        await cancelChat(opt.base, requestId).catch((e) => say(`# cancel 失败:${e.message}`));
      }
      await sleep(POLL_INTERVAL_S * 1000);
    }
  };

  let finalError = null;
  try {
    stopReason = await pollLoop();
  } catch (e) {
    finalError = e;
  }
  process.off('SIGINT', onSigint);
  if (lastProgress) process.stderr.write('\n');

  const stoppedAtMs = Date.now();
  // stop_reason 不篡改:脚本错误路径保持 daemon 侧真实值;只有「中断后
  // cancel 落定窗内没等到」才按已发 cancel 报 cancelled
  if (stopReason == null && interrupted && !finalError) {
    stopReason = 'cancelled';
    say(`# 中断后 ${CANCEL_SETTLE_S}s 内未见 stop_reason 落定,按已发 cancel 报 cancelled`);
  }

  // 终态读转录(无论成败);失败路径也导出(post-mortem)
  let loaded = null;
  try {
    loaded = await loadSession(opt.base, sessionId);
  } catch (e) {
    if (!finalError) finalError = e; else say(`# 转录读取失败(叠加):${e.message}`);
  }
  if (loaded) {
    const outPath = opt.out || defaultTranscriptPath(opt.topic);
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    const note = finalError
      ? `脚本错误:${finalError.message}`
      : (interrupted ? (stopReason === 'cancelled' ? '超时/手动中断,部分转录' : '中断竞态') : undefined);
    fs.writeFileSync(outPath, renderTranscript({
      session: { ...loaded.session, ...(stopReason ? { stop_reason: stopReason } : {}) },
      messages: loaded.messages,
      startedAtMs, stoppedAtMs, note,
      modelNames: Object.fromEntries(models.map((m) => [m.id, m.displayName || m.modelName])),
    }));
    emit(`# 转录: ${outPath}`);
    if (loaded.session.discussion_summary) emit(`# discussion_summary:\n${loaded.session.discussion_summary}`);
  }

  if (finalError) {
    emit(`# 脚本错误:${finalError.message}`);
    return EXIT.scriptError;
  }

  if (opt.cleanup && stopReason === 'group_chat_end') {
    await deleteSession(opt.base, sessionId);
    say('# --cleanup:session 已删除(转录保留)');
  } else if (opt.cleanup && stopReason !== 'group_chat_end') {
    say(`# --cleanup 只作用于成功路径;本次 stop_reason=${stopReason},session 保留供 post-mortem`);
  }

  if (stopReason === 'interrupted') {
    emit('# 讨论 daemon 进程级中断(崩溃/被杀);checkpoint 已落库,可续跑:');
    emit(`#   curl -X POST ${opt.base}/api/v1/agent/resume_group_chat -H 'content-type: application/json' -d '{"session_id":"${sessionId}"}'`);
  }
  const code = EXIT_BY_STOP_REASON[stopReason] ?? EXIT.scriptError;
  emit(`# stop_reason=${stopReason} → exit ${code}`);
  return code;
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ---------------------------------------------------------------------------
// 内省子命令(建群所需全部运行时事实;LLM 不读文档、查脚本)
// ---------------------------------------------------------------------------

async function cmdProjects(base) {
  await checkDaemon(base);
  // 含隐藏项(GUI 侧栏隐藏的项目照样能当审议 cwd;排除会重演 resolveProject 撞 400 的困惑)
  const list = await api(base, 'projects/list_projects', { body: { filter: { hidden: true } } });
  process.stdout.write('project_id                           path\n');
  for (const p of list) process.stdout.write(`${p.id.padEnd(36)} ${p.path}${p.hidden ? '  (隐藏)' : ''}\n`);
  process.stdout.write(`\n共 ${list.length} 个。run --project <path> 按物理路径匹配,miss 自动创建。\n`);
}

async function cmdModels(base) {
  await checkDaemon(base);
  const models = await listModels(base);
  process.stdout.write('model UUID(实际引用值)                       modelName              provider\n');
  for (const m of models) {
    process.stdout.write(`${m.id.padEnd(42)} ${(m.modelName || '').padEnd(20)} ${m.providerDisplayName || ''}\n`);
  }
  process.stdout.write(`
共 ${models.length} 个。CLI 传 --moderator-model / --set name.model= 时 UUID 与名字(modelName/displayName)都收,
脚本统一解析成 UUID 再进 metadata —— daemon 侧 catalog 只认 UUID(group_chat_loop.rs resolve_provider,无名字 fallback)。
注意:providers 域返回 camelCase(DAEMON-API.md §2 同款实踩点)。\n`);
}

function cmdPresets() {
  for (const [name, preset] of Object.entries(PRESETS)) {
    process.stdout.write(`\n== ${name} — ${preset.description}\n`);
    process.stdout.write(`   moderator: ${preset.moderator_model}\n`);
    for (const x of preset.participants) {
      process.stdout.write(`   - ${x.name} / ${x.model} / persona ${x.persona_md.length} 字符\n`);
    }
  }
  process.stdout.write(`
覆盖语法:
  --participants '<json>'            整名单替换([{name, model, persona_md?}])——增删参与者的唯一方式
  --set <name>.model=<id>            单人换模型(可重复)
  --set <name>.persona=@file|文本     单人换 persona(可重复)
  --moderator-model <id>             换主持人
预设单一事实源是 scripts/group-chat-presets.json(M4a R7,定时任务与脚本共享);个性化靠覆盖,不靠改脚本。\n`);
}

function printRunHelp() {
  process.stdout.write(`run — 发起一场群聊审议(一条命令:建群 → 发题 → 轮询 → 导转录)

  --project <path>        审议对象的项目目录(证据基地;默认当前目录;miss 自动创建)
  --topic-file <path>     议题文件(主推;长议题/含引号转义都走文件)
  --topic <text>          议题内联(短议题用)
  --preset <name>         review / arch / retro(见 presets 子命令)
  --participants <json>   整名单替换(与 --preset 二选一;增删参与者也走它)
  --moderator-model <id>  主持人模型(默认取预设)
  --set <name>.model=<id>            单人换模型(可重复)
  --set <name>.persona=@file|文本     单人换 persona(可重复)
  --timeout <seconds>     默认 1800;超时 cancel 停编排、保 session、导部分转录
  --out <path>            转录落点(默认 <仓库根>/out/group-chat-<slug>-<ts>.md)
  --quiet                 静默进度(cron 用);session id/转录路径/终态仍打 stderr
  --cleanup               成功收官后删 session(中断现场永不删)
  --dry-run               打印静态请求模板,零网络(参数组装回归在单测)
  --base <url>            daemon 地址(默认 \${EVERLASTING_BASE:-http://127.0.0.1:7456})

进度粒度:轮询 10s 一拍,时间戳精度 ±10s;发言级实时进度不做(需 SSE,会破坏
无人值守 8s 快拒语义)。

退出码:0 group_chat_end / 2 max_rounds / 3 cancelled / 4 error / 1 脚本自身错误
(脚本错误同样导出部分转录,现场不丢)\n`);
}

// ---------------------------------------------------------------------------
// CLI 壳
// ---------------------------------------------------------------------------

function usage() {
  process.stdout.write(`group-chat-run — 群聊审议驱动(GCE-M1;docs/GROUP-CHAT-API-ROADMAP.md §2)

  node scripts/group-chat-run.mjs projects | models | presets   # 建群三要素内省
  node scripts/group-chat-run.mjs run [options]                 # 发起审议(--help 详)
  node scripts/group-chat-run.mjs run --help                    # run 全量参数

环境:EVERLASTING_BASE(daemon 地址,默认 http://127.0.0.1:7456)\n`);
}

async function main() {
  const [sub, ...rest] = process.argv.slice(2);
  switch (sub) {
    case 'projects': return cmdProjects(DEFAULT_BASE);
    case 'models': return cmdModels(DEFAULT_BASE);
    case 'presets': return cmdPresets();
    case 'run': return run(rest);
    case '--help': case '-h': case undefined: return usage();
    default: throw new Error(`未知子命令 "${sub}"`);
  }
}

// import 直连(M2 MCP/测试)时跳过 CLI 壳
if (process.argv[1] && path.resolve(process.argv[1]) === SCRIPT_PATH) {
  main()
    .then((code) => process.exit(code ?? 0))
    .catch((e) => {
      process.stderr.write(`[group-chat-run] ${e.message}\n`);
      process.exit(EXIT.scriptError);
    });
}
