#!/usr/bin/env node
// group-chat-mcp.mjs — GCE-M2 MCP 接口层(四工具:召集/轮询/取结论/止损;
// M3 增打断/注入两工具,控制面三权齐:cancel 硬停 / interrupt 收束 / inject 注入)
//
// 三层架构(M1 定形)中的薄包装:编排语义(建群契约/模型解析/终态判定/
// 转录渲染)全部 import 自 group-chat-run.mjs(AC②,不是两套);本文件
// 只做「翻译 + 记账 + 惰性转录」。设计定案见任务 09-06-gce-m2-mcp-interface
// 的 design.md(D1 Node+SDK / D2 stdio / D3 四工具+预算 / D5 created_via)。
//
// 语义约束(勿改):
// - 工具调用绝不阻塞:start 立即返回 session_id,进度靠轮询;一场 5-15
//   分钟、数十万 token——成本闸写死在工具描述里(R3)。
// - 转录惰性导出:终态首次被 status/result 观测时落 <讨论 cwd>/out/
//   (design §6 有意分叉:M1 CLI 落引擎仓库根);导出失败降级
//   transcript_path:null + 警告,status 永不因导出报错(评审 P2-1)。
// - 记账(session→request_id/project_id/cwd/topic/started_at_ms)写穿
//   XDG state 文件:stdio server 随宿主会话生灭,讨论 5-15 分钟跨进程
//   存活是常态;busy 只在 list_sessions(按 project_id)富化,重启兜底
//   链 = 记账命中→list_sessions;全 miss→load_session 取 project_id→回查。
// - 主持人恒取 preset 的 moderator_model(participants 整名单只换名单,
//   镜像 M1 CLI run.mjs:416;评审 P1-1 定案)。
// - 纯逻辑区零 SDK import —— 单测可脱离 SDK 跑;stdout 是协议通道,
//   诊断只走 stderr。

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

import {
  PRESETS, DEFAULT_BASE, fetchFailDetail,
  resolveParticipants, validateModelRefs, buildCreateSessionBody, buildChatBody,
  defaultTranscriptPath, renderTranscript, injectGuardDecision, interpretAcceptance,
  resolveProject, listModels, createSession, fireChat, pollSession, loadSession, cancelChat,
  preemptGroupChat, listTurnTraces, aggregateTokens,
} from './group-chat-run.mjs';

// ---------------------------------------------------------------------------
// 纯逻辑区(零 SDK import)
// ---------------------------------------------------------------------------

function stateFilePath() {
  const root = process.env.XDG_STATE_HOME || path.join(os.homedir(), '.local', 'state');
  return path.join(root, 'dev.everlasting.app', 'mcp-discussions.json');
}

/** 记账:内存 Map + XDG state 文件写穿(tmp+rename 原子写;miss 时读文件兜底)。 */
export function createLedger({ file = stateFilePath() } = {}) {
  const mem = new Map();
  let fileLoaded = false;
  const read = () => {
    try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return {}; }
  };
  const write = (obj) => {
    try {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      const tmp = `${file}.tmp-${process.pid}`;
      fs.writeFileSync(tmp, JSON.stringify(obj));
      fs.renameSync(tmp, file);
    } catch { /* 记账持久化失败不阻断主流程:内存仍准,cancel 兜底降级 */ }
  };
  return {
    get(sessionId) {
      if (mem.has(sessionId)) return mem.get(sessionId);
      if (!fileLoaded) {
        fileLoaded = true;
        for (const [k, v] of Object.entries(read())) if (!mem.has(k)) mem.set(k, v);
      }
      return mem.get(sessionId) || null;
    },
    set(sessionId, entry) {
      mem.set(sessionId, entry);
      write(Object.fromEntries(mem));
    },
  };
}

/** 会话级终态(GC1/GC2):busy=false 且 stop_reason 非空。
 * 枚举 group_chat_end/max_rounds/cancelled/error/interrupted(GCE P1a
 * 09-06 起:interrupted = 进程级中断,boot sweep 标记,可经
 * resume_group_chat 续跑);轮级跳轮值(nominee_unknown/
 * participant_unresolved)不落 session 终态列,此处不遇。 */
export function isTerminal(session) {
  return Boolean(session) && !session.busy && session.stop_reason != null;
}

/** 实际 daemon 依赖(测试注入 mock 替换;BASE 支持 EVERLASTING_BASE 同 M1)。 */
export function realDeps(overrides = {}) {
  const base = process.env.EVERLASTING_BASE || DEFAULT_BASE;
  return {
    base,
    resolveProject: (cwd) => resolveProject(base, cwd),
    listModels: () => listModels(base),
    createSession: (body) => createSession(base, body),
    fireChat: (body) => fireChat(base, body),
    pollSession: (projectId, sessionId) => pollSession(base, projectId, sessionId),
    loadSession: (sessionId) => loadSession(base, sessionId),
    listTurnTraces: (sessionId) => listTurnTraces(base, sessionId),
    cancelChat: (requestId) => cancelChat(base, requestId),
    preemptGroupChat: (sessionId) => preemptGroupChat(base, sessionId),
    ...overrides,
  };
}

/** daemon 不可达/HTTP 错误统一翻译(嵌套消费语义:M1 fetchFailDetail 单源)。 */
export function daemonError(e) {
  const detail = e && e.message ? e.message : String(e);
  return `daemon 调用失败:${detail}${/daemon 不可达|fetch failed|ECONNREFUSED/i.test(detail) ? `;原始网络错误:${fetchFailDetail(e.cause || e)};先确认 daemon 在跑(scripts/daemon.sh)` : ''}`;
}

/** start 编排链:M1 导出全组合;moderator 恒取 preset 预设值。 */
export async function coreStart(deps, ledger, { topic, cwd, preset = 'review', participants, tokenBudget }) {
  if (!topic || !String(topic).trim()) throw new Error('缺议题:topic(议题质量直接决定产出质量,不要把答案写进问题)');
  if (!cwd) throw new Error('缺工作目录:cwd(讨论的证据基地)');
  if (tokenBudget !== undefined && (!Number.isInteger(tokenBudget) || tokenBudget <= 0)) {
    throw new Error('token_budget 必须是正整数(不限请省略该参数)');
  }

  const roster = resolveParticipants({
    preset,
    participantsJson: participants ? JSON.stringify(participants) : undefined,
    set: [],
  });
  const moderatorModel = PRESETS[preset]?.moderator_model;
  if (!moderatorModel) throw new Error(`未知预设 "${preset}";可用:${Object.keys(PRESETS).join(' / ')}`);

  const models = await deps.listModels();
  const norm = validateModelRefs(models, { moderatorModel, participants: roster });
  const proj = await deps.resolveProject(path.resolve(cwd));
  const session = await deps.createSession(buildCreateSessionBody({
    projectId: proj.id,
    projectPath: path.resolve(cwd),
    moderatorModel: norm.moderatorModelId,
    participants: norm.participants,
    createdVia: 'mcp',
    tokenBudget,
  }));
  const requestId = `gcmcp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  await deps.fireChat(buildChatBody({ requestId, sessionId: session.id, topic }));
  ledger.set(session.id, {
    request_id: requestId, project_id: proj.id, cwd: path.resolve(cwd),
    topic, started_at_ms: Date.now(),
  });
  return {
    session_id: session.id,
    request_id: requestId,
    hint: 'started — poll discussion_status(session_id); after stop_reason is set, read discussion_result. Expect 5-15 min.',
  };
}

/** status/result 共用的会话查找两级链(评审 P1-2):记账 → list_sessions;
 * 全 miss(手抄 session_id)→ load_session 取 project_id → 回查。
 * load_session 的 busy 恒 false(list_sessions_inner 才做富化),不可信。 */
async function findSession(deps, ledger, sessionId) {
  const entry = ledger.get(sessionId);
  if (entry?.project_id) {
    const s = await deps.pollSession(entry.project_id, sessionId);
    if (s) return { entry, summary: s };
  }
  const loaded = await deps.loadSession(sessionId);
  if (!loaded) throw new Error(`session 不存在:${sessionId}`);
  const s = await deps.pollSession(loaded.session.project_id, sessionId);
  if (!s) throw new Error(`session 不存在:${sessionId}`);
  return { entry, summary: s };
}

/** 惰性转录导出(幂等;status/result 双入口)。失败降级不抛错(评审 P2-1)。 */
export async function ensureTranscript(deps, ledger, sessionId, { entry, summary, loaded }) {
  const existing = entry?.transcript_path;
  if (existing && fs.existsSync(existing)) return { transcript_path: existing };

  const full = loaded || await deps.loadSession(sessionId);
  if (!full) return { transcript_path: null, transcript_warning: 'session 已不存在,无法导出转录' };
  let modelNames = {};
  try {
    modelNames = Object.fromEntries((await deps.listModels()).map((m) => [m.id, m.displayName || m.modelName]));
  } catch { /* 目录拿不到就用 UUID 原样,渲染不因目录失败而断 */ }
  const root = path.resolve(full.session.current_cwd || entry?.cwd || os.tmpdir());
  const topic = entry?.topic || (full.messages[0]?.text ? String(full.messages[0].text).slice(0, 40) : 'discussion');
  const startedAtMs = entry?.started_at_ms || Date.parse(full.session.created_at || '') || Date.now();
  const target = defaultTranscriptPath(topic, root);
  try {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, renderTranscript({
      session: full.session,
      messages: full.messages,
      startedAtMs,
      stoppedAtMs: Date.now(),
      modelNames,
    }));
  } catch (e) {
    return { transcript_path: null, transcript_warning: `转录导出失败(${e.message});讨论结论不受影响,可重试 discussion_result` };
  }
  ledger.set(sessionId, { ...(entry || {}), project_id: entry?.project_id || full.session.project_id, transcript_path: target });
  return { transcript_path: target };
}

export async function coreStatus(deps, ledger, sessionId) {
  const { entry, summary } = await findSession(deps, ledger, sessionId);
  const elapsed_s = entry?.started_at_ms ? Math.round((Date.now() - entry.started_at_ms) / 1000) : null;
  const out = {
    busy: summary.busy,
    stop_reason: summary.stop_reason ?? null,
    elapsed_s,
  };
  if (isTerminal(summary)) {
    // status 是廉价轮询(无轮次/消息字段,评审 P2-2);终态观测点触发惰性转录
    Object.assign(out, await ensureTranscript(deps, ledger, sessionId, { entry, summary }));
  }
  return out;
}

/** gce-m4c:result 的 tokens 键(纯函数 aggregateTokens 的 IO 壳):
 * listTurnTraces 失败 → 空对象(键整体省略),既有 stats 字段不受影响。 */
async function computeTokens(deps, sessionId, messages) {
  try {
    const traces = await deps.listTurnTraces(sessionId);
    const { total, per_speaker } = aggregateTokens(traces, messages);
    return { tokens: { total, per_speaker } };
  } catch {
    return {};
  }
}

export async function coreResult(deps, ledger, sessionId) {
  const { entry, summary } = await findSession(deps, ledger, sessionId);
  if (!isTerminal(summary)) {
    const err = new Error(`still running (busy=${summary.busy}, stop_reason not set) — poll discussion_status; expect 5-15 min total`);
    err.isToolError = true;
    throw err;
  }
  const loaded = await deps.loadSession(sessionId);
  if (!loaded) throw new Error(`session 不存在:${sessionId}`);
  const transcript = await ensureTranscript(deps, ledger, sessionId, { entry, summary, loaded });
  let modelNames = {};
  try {
    modelNames = Object.fromEntries((await deps.listModels()).map((m) => [m.id, m.displayName || m.modelName]));
  } catch { /* 同 ensureTranscript:目录失败降级 UUID 原样 */ }
  const meta = typeof loaded.session.metadata === 'string' ? JSON.parse(loaded.session.metadata) : loaded.session.metadata;
  const disp = (id) => modelNames[id] || id;
  const out = {
    stop_reason: summary.stop_reason,
    summary: loaded.session.discussion_summary || null,
    roster: {
      moderator: disp(loaded.session.model || loaded.session.model_id || ''),
      participants: (meta?.participants || []).map((p) => `${p.name}/${disp(p.model)}`),
    },
    stats: {
      messages: loaded.messages.length,
      elapsed_s: entry?.started_at_ms ? Math.round((Date.now() - entry.started_at_ms) / 1000) : null,
    },
    // gce-m4c:计费核算(per-speaker + total,与 stop_reason=budget 预算同
    // 口径四字段求和)。聚合失败(list_turn_traces 不可用)→ 整键省略,
    // 不污染既有 stats 字段(M2 惰性转录降级同款设计)。
    ...(await computeTokens(deps, sessionId, loaded.messages)),
    ...transcript,
  };
  if (!loaded.session.discussion_summary) {
    out.summary_warning = '正常收官但 discussion_summary 缺失(moderator 未走 end_discussion);读转录尾段人工收束';
  }
  return out;
}

export async function coreCancel(deps, ledger, sessionId) {
  const entry = ledger.get(sessionId);
  if (!entry?.request_id) {
    const err = new Error(`无此讨论的记账(可能是本 server 进程重启前的历史 session):${sessionId};若确认在跑,可用 M1 CLI 语义 cancel_chat(request_id) 或等自然收官`);
    err.isToolError = true;
    throw err;
  }
  try {
    await deps.cancelChat(entry.request_id);
    return { cancelled: true, session_id: sessionId, note: '编排已停,session 保留(部分转录照常可导出)' };
  } catch (e) {
    // 已终态后 cancel 报错 → 幂等成功(cancel 语义:停编排,不是删数据)
    const { summary } = await findSession(deps, ledger, sessionId).catch(() => ({ summary: null }));
    if (!summary || isTerminal(summary)) {
      return { already_finished: true, stop_reason: summary?.stop_reason ?? null, session_id: sessionId };
    }
    throw new Error(daemonError(e));
  }
}

/** GCE-M3 interrupt_discussion:session 域收束打断(preempt 端点 1:1;
 * 与 coreCancel 的 rid 域硬停相对——收束等在途发言完并落 summary)。
 * 无进行中讨论 → 端点报错原样透传(无副作用,不需要前置 guard)。 */
export async function coreInterrupt(deps, ledger, sessionId) {
  const { preempted } = await deps.preemptGroupChat(sessionId);
  return {
    interrupted: preempted === true,
    session_id: sessionId,
    hint: 'Wrap-up in progress: the in-flight speaker finishes, then the moderator rounds off (~1-3 min). Poll discussion_status; after it turns terminal (stop_reason=preempted, or group_chat_end if the discussion finished naturally in the same instant), read discussion_result.',
  };
}

/** GCE-M3 inject_message:往进行中的讨论注入用户消息(controls 缓冲,
 * 下一 moderator 轮可见,讨论不死)。主防护 = 前置 busy guard(评审
 * P1-1:空闲/已收官群聊一旦 fireChat 会重启编排器并无条件抹旧场
 * summary,cancel 救不回 → 非 busy 根本不发起);fireChat 后 acceptance
 * 非 injected → 自有 rid 即时 cancel 竞态兜底 + 语义报错。 */
export async function coreInject(deps, ledger, { session_id: sessionId, text }) {
  const trimmed = String(text ?? '').trim();
  if (!trimmed) {
    const err = new Error('缺注入文本:text(注入只收文本;纯图片注入不支持)');
    err.isToolError = true;
    throw err;
  }
  const { summary } = await findSession(deps, ledger, sessionId);
  const guard = injectGuardDecision(summary);
  if (!guard.allowed) {
    const err = new Error(guard.reason);
    err.isToolError = true;
    throw err;
  }
  const requestId = `gcinject-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const acceptance = await deps.fireChat(buildChatBody({ requestId, sessionId, topic: trimmed }));
  const verdict = interpretAcceptance(acceptance);
  if (verdict.kind !== 'injected') {
    await deps.cancelChat(requestId).catch(() => { /* 止损尽力而为;语义错误照报 */ });
    const err = new Error(`目标不是进行中的群聊讨论(acceptance=${verdict.status});已止损取消本次请求。发起新讨论请用 start_discussion。`);
    err.isToolError = true;
    throw err;
  }
  return {
    injected: true,
    session_id: sessionId,
    hint: 'Injected — lands as [用户插入] in the next moderator round; the discussion continues. Poll discussion_status as usual.',
  };
}

// ---------------------------------------------------------------------------
// 工具定义区(description+inputSchema 即产品;预算见 TOOLS_BUDGET_CHARS,
// AC4 单测按 wire 上的 JSON Schema 实测锁上限 —— 宿主注入 LLM context 的
// 就是 listTools 返回的 schema,这才是预算的地面真值)
// ---------------------------------------------------------------------------

export const TOOLS_BUDGET_CHARS = 3200; // AC4:六工具 name+description+inputSchema(wire JSON Schema)合计字符上限;gce-m4c(09-08)加 token_budget 参后实测 3115,09-09 加 fe_review 预设(enum+描述)后 3162,余量 ~38 字符——再扩 description 大概率要升锁(升锁须过评审,同步本注释 + AC4 断言 + spec)

/** Zod shape(SDK 1.30 registerTool 只收 Zod;内部转 JSON Schema 上 wire)。
 * description 克制:D3 约束 —— 只留「干什么/成本闸/不阻塞」三件事。 */
export function buildToolShapes(z) {
  return {
    start_discussion: {
      topic: z.string().describe('The question; evidence-backed, do not bake the answer in'),
      cwd: z.string().describe('Project dir as evidence base'),
      preset: z.enum(['review', 'fe_review', 'arch', 'retro']).optional().describe('Participant preset'),
      participants: z.array(z.object({
        name: z.string(),
        model: z.string().describe('Catalog name or UUID'),
        persona_md: z.string().optional(),
      })).optional().describe('Full roster, replaces preset roster (moderator unchanged)'),
      token_budget: z.number().int().positive().optional().describe('Billed-token ceiling (input+output+cache_creation+cache_read); exceeded → halts at next round head with stop_reason=budget. Omit = unlimited'),
    },
    discussion_status: { session_id: z.string() },
    discussion_result: { session_id: z.string() },
    cancel_discussion: { session_id: z.string() },
    interrupt_discussion: { session_id: z.string() },
    inject_message: {
      session_id: z.string(),
      text: z.string().min(1).describe('User message text; lands as [用户插入] in the next moderator round'),
    },
  };
}

export const TOOLS = [
  {
    name: 'start_discussion',
    description: 'Convene a multi-LLM group deliberation on a topic. Costly: 5-15 min, hundreds of thousands of tokens. Returns immediately with session_id — poll discussion_status, read conclusions via discussion_result. Presets: review (arch+product+backend), fe_review (arch+product+frontend), arch (2-person), retro (product+outsider).',
    shapeKey: 'start_discussion',
  },
  {
    name: 'discussion_status',
    description: 'Check a discussion: busy=true running; busy=false + stop_reason (group_chat_end|max_rounds|cancelled|error) = finished.',
    shapeKey: 'discussion_status',
  },
  {
    name: 'discussion_result',
    description: 'Read a finished discussion\'s conclusion (errors while running — poll discussion_status first). Returns summary, roster, stats, transcript path.',
    shapeKey: 'discussion_result',
  },
  {
    name: 'cancel_discussion',
    description: 'Stop a running discussion (orchestration stops, session kept).',
    shapeKey: 'cancel_discussion',
  },
  {
    name: 'interrupt_discussion',
    description: 'Gracefully stop a running discussion: in-flight speaker finishes, the moderator wraps up with a summary, stop_reason=preempted. Returns immediately; poll discussion_status (~1-3 min), then read discussion_result.',
    shapeKey: 'interrupt_discussion',
  },
  {
    name: 'inject_message',
    description: 'Inject a user message into a RUNNING discussion; the next moderator round sees it and the discussion continues. Errors if the session is not busy — use start_discussion to convene a new one.',
    shapeKey: 'inject_message',
  },
];

// ---------------------------------------------------------------------------
// SDK 接线区(薄壳:handler 只做参数透传 + 错误翻译)
// ---------------------------------------------------------------------------

function textResult(obj) {
  return { content: [{ type: 'text', text: JSON.stringify(obj, null, 2) }] };
}

function errorResult(e) {
  const payload = e.isToolError
    ? { error: e.message }
    : { error: e.message, hint: 'daemon 调用链问题先确认 daemon 在跑(scripts/daemon.sh)' };
  return { content: [{ type: 'text', text: JSON.stringify(payload) }], isError: true };
}

export async function createServer({ server, deps = realDeps(), ledger = createLedger() } = {}) {
  const { z } = await import('zod');
  const shapes = buildToolShapes(z);
  const handlers = {
    start_discussion: ({ topic, cwd, preset, participants, token_budget }) => coreStart(deps, ledger, { topic, cwd, preset, participants, tokenBudget: token_budget }),
    discussion_status: ({ session_id }) => coreStatus(deps, ledger, session_id),
    discussion_result: ({ session_id }) => coreResult(deps, ledger, session_id),
    cancel_discussion: ({ session_id }) => coreCancel(deps, ledger, session_id),
    interrupt_discussion: ({ session_id }) => coreInterrupt(deps, ledger, session_id),
    inject_message: ({ session_id, text }) => coreInject(deps, ledger, { session_id, text }),
  };
  for (const tool of TOOLS) {
    const handler = handlers[tool.name];
    // SDK 1.30 三参制 registerTool(name, config, cb);inputSchema 收 Zod
    // shape(内部转 JSON Schema 上 wire);cb 直收解析后的参数对象。
    server.registerTool(tool.name, { description: tool.description, inputSchema: shapes[tool.shapeKey] }, async (args) => {
      try {
        return textResult(await handler(args || {}));
      } catch (e) {
        return errorResult(e);
      }
    });
  }
  return server;
}

// ---------------------------------------------------------------------------
// main:直接运行 = stdio server(宿主 spawn;诊断只走 stderr)
// ---------------------------------------------------------------------------

const isMain = (() => {
  try {
    return process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch { return false; }
})();

if (isMain) {
  const { McpServer } = await import('@modelcontextprotocol/sdk/server/mcp.js');
  const { StdioServerTransport } = await import('@modelcontextprotocol/sdk/server/stdio.js');
  const server = await createServer({ server: new McpServer({ name: 'everlasting-group-chat', version: '1.0.0' }) });
  await server.connect(new StdioServerTransport());
  process.stderr.write(`[group-chat-mcp] stdio server up (base=${process.env.EVERLASTING_BASE || DEFAULT_BASE})\n`);
}
