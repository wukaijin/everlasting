// lib/discuss.mjs — `evl discuss` 命令面(design §4:动词分发 + 全链编排)。
//
// 分层(评审 09-19 定案):编排唯一归属 daemon(建群/轮次/preset 合并),CLI
// 只做运输——flag 翻译 + 轮询节奏 + 退出码翻译。经 lib/mcp.mjs 调 daemon
// `POST /mcp` 的 tools/call;禁 import scripts/group-chat-run.mjs(编排单源
// 纪律,.trellis/spec/cli/index.md,不复活已退役的 JS 编排双实现)。
//
// 三处判据(评审 09-19,mcp.rs:1014/1062 实读,实现勿"简化"掉):
// - 变化 = `wait_timed_out !== true`(该键 daemon 仅在 wait 超时才写,变化即返
//   时键缺失;按 `== false` 实现会把变化检测整体反转);
// - 终态短路先于窗口循环(busy==false && stop_reason != null → 秒返);
// - text 输出按字段存在性取(wait 隐含 detail,mcp.rs want_progress,不按
//   --detail flag 取)。
//
// 退出码(design §6,值域开放集):0 group_chat_end/max_rounds/cancelled/
// preempted;1 不可达/协议错/语义错/interrupted(可续跑态)/表外未知非空;
// 2 error/nominee_unknown/participant_unresolved;3 SIGINT;6 budget(唯一
// 借档,M1 同款);7 --timeout 到点(**不 cancel**,讨论仍在跑,勿重跑);64 用法错。
import process from 'node:process';
import { EvlError, EXIT } from './api.mjs';
import { callMcpTool } from './mcp.mjs';
import { UsageError } from './args.mjs';
import { ensureTrailingNewline, formatTable, toJsonLine, truncate } from './format.mjs';

/** 控制动词(第一位置参数);无动词 = 全链。 */
export const VERBS = ['start', 'status', 'result', 'cancel', 'interrupt', 'inject', 'presets'];

/** MCP 单次长轮询上限(秒)——daemon 契约 1..25(mcp.rs MAX_WAIT_SECONDS,
 * 避宿主 30s 工具超时);CLI 侧循环无此罚(HTTP 往返,无每 call 一 LLM turn)。 */
export const MCP_POLL_WAIT_S = 25;

/** budget → 6:evl 六档契约外唯一借档(M1 同款,commit 772e823d 先例)。 */
export const EXIT_BUDGET = 6;

const EXIT_OK_REASONS = new Set(['group_chat_end', 'max_rounds', 'cancelled', 'preempted']);
const EXIT_ERROR_REASONS = new Set(['error', 'nominee_unknown', 'participant_unresolved']);

// ── 纯函数(node --test 覆盖)────────────────────────────────────────────

/**
 * stop_reason → 退出码。值域按开放集处理:具名档查表,表外未知非空与
 * null(终态却无值,防御)都落 1——stderr 侧由 stopReasonNote 给出区分文案。
 */
export function stopReasonExitCode(reason) {
  if (reason == null || reason === '') return EXIT.scriptError;
  if (EXIT_OK_REASONS.has(reason)) return EXIT.ok;
  if (EXIT_ERROR_REASONS.has(reason)) return EXIT.chatError; // 2 = 异常收场族
  if (reason === 'budget') return EXIT_BUDGET;
  return EXIT.scriptError; // interrupted(可续跑态)与表外未知非空值
}

/** stop_reason → stderr 附加说明(全链收尾用;具名正常档返回 null 不打)。 */
export function stopReasonNote(reason) {
  if (reason == null) return 'stop_reason=null(终态却无收场值,防御档;详情查转录或 daemon 日志)';
  if (reason === '') return 'stop_reason 为空串(防御档;详情查转录或 daemon 日志)';
  if (reason === 'interrupted') {
    return 'interrupted:崩溃恢复标记的可续跑态(非未知异常);续观察:evl discuss status <sid> --wait 540';
  }
  if (!EXIT_OK_REASONS.has(reason) && !EXIT_ERROR_REASONS.has(reason) && reason !== 'budget') {
    return `未知 stop_reason:"${reason}"(值域开放集,daemon 可能新增值;按表外非空值处理,退 1)`;
  }
  return null;
}

/**
 * 进度行 diff:比较 messages/last_speaker/tokens.total/busy/stop_reason,
 * 有变化(或 prev 为空 = 首拍)产出一行,无变化返回 null。
 */
export function diffProgressLine(prev, next) {
  const sig = (s) =>
    JSON.stringify([
      s?.messages ?? null,
      s?.last_speaker ?? null,
      s?.tokens?.total ?? null,
      s?.busy ?? null,
      s?.stop_reason ?? null,
    ]);
  if (prev != null && sig(prev) === sig(next)) return null;
  const parts = [];
  if (next?.messages != null) parts.push(`msg ${next.messages}`);
  parts.push(`last ${next?.last_speaker ?? '-'}`);
  if (next?.tokens?.total != null) parts.push(`tokens ${next.tokens.total}`);
  return `[discuss] ${parts.join(' · ')} (elapsed ${next?.elapsed_s ?? '?'}s)`;
}

/**
 * 长轮询切片:距 deadline 的剩余秒数裁到 1..cap(MCP 单次 wait_seconds 契约
 * 上限 25)。剩余不足 1s 时仍给 1(daemon 侧 1s 上界,略过线可接受)。
 */
export function waitSlice(deadlineMs, nowMs, cap = MCP_POLL_WAIT_S) {
  const remainS = Math.ceil((deadlineMs - nowMs) / 1000);
  if (remainS <= 1) return 1;
  return Math.min(cap, remainS);
}

// ── 文本格式化(stdout 只出数据;字段存在性取,不按 flag 取)──────────────

/** status 快照 → text(逐字段存在性输出;wait 隐含 detail 时字段才会出现)。 */
export function formatStatusText(snap) {
  const lines = [`busy: ${snap?.busy ?? '?'}`];
  if (snap?.stop_reason != null) lines.push(`stop_reason: ${snap.stop_reason}`);
  if (snap?.elapsed_s != null) lines.push(`elapsed_s: ${snap.elapsed_s}`);
  if (snap?.messages != null) lines.push(`messages: ${snap.messages}`);
  if (snap?.last_speaker != null) lines.push(`last_speaker: ${snap.last_speaker}`);
  if (snap?.tokens?.total != null) lines.push(`tokens: ${snap.tokens.total}`);
  if (snap?.token_budget != null) lines.push(`token_budget: ${snap.token_budget}`);
  if (snap?.transcript_path) lines.push(`transcript: ${snap.transcript_path}`);
  if (snap?.wait_timed_out === true) lines.push('wait_timed_out: true');
  return lines.join('\n');
}

/** result 载荷 → text:summary + roster/stats/tokens 摘要 + transcript 落点 +
 * 末行 stop_reason 标记(max_rounds→0 时 text 消费方靠它分辨轮帽截断)。 */
export function formatResultText(payload) {
  const lines = [];
  if (payload?.summary) lines.push(String(payload.summary).trimEnd());
  const roster = payload?.roster;
  if (roster) {
    lines.push('');
    lines.push(`moderator: ${roster.moderator ?? '-'}`);
    const participants = roster.participants ?? [];
    lines.push(`participants(${participants.length}): ${participants.join(', ')}`);
  }
  const stats = payload?.stats;
  if (stats) lines.push(`stats: messages ${stats.messages ?? '?'} · elapsed ${stats.elapsed_s ?? '?'}s`);
  if (payload?.tokens) lines.push(`tokens: ${payload.tokens.total}`);
  if (payload?.transcript_path) lines.push(`transcript: ${payload.transcript_path}`);
  for (const k of ['detail_warning', 'summary_warning', 'transcript_warning']) {
    if (payload?.[k]) lines.push(`warning: ${truncate(payload[k], 200)}`);
  }
  lines.push(`stop_reason: ${payload?.stop_reason ?? 'null'}`);
  return lines.join('\n');
}

/** cancel/interrupt/inject 载荷 → text(键值逐行;字符串原样,其余 JSON 化)。 */
function formatPayloadText(payload) {
  return Object.entries(payload ?? {})
    .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
    .join('\n');
}

// ── IO 编排 ─────────────────────────────────────────────────────────────

function topicOf(positionals) {
  const topic = positionals.join(' ').trim();
  if (topic === '') {
    throw new UsageError('discuss 缺议题文本(用法:evl discuss "<topic>" [flags];动词:evl discuss --help)');
  }
  return topic;
}

function sidOf(positionals, verb) {
  const sid = positionals[0];
  if (!sid) {
    throw new UsageError(`discuss ${verb} 缺 session id(用法:evl discuss ${verb} <sid> [args];id 在建群时的 stderr)`);
  }
  return sid;
}

/** start_discussion 入参:仅显式给出的 flag 才带键(preset 缺省 review 由 daemon 定)。 */
function startArgs(flags, topic) {
  return {
    topic,
    cwd: flags.cwd ?? process.cwd(),
    ...(flags.preset != null ? { preset: flags.preset } : {}),
    ...(flags.roster != null ? { participants: flags.roster } : {}),
    ...(flags.tokenBudget != null ? { token_budget: flags.tokenBudget } : {}),
  };
}

/**
 * status 快照获取(动词用)。无 --wait 单次;--wait n 外层窗口循环(内部
 * wait_seconds ≤ 25):终态短路先判,变化(wait_timed_out !== true)即返,
 * 窗口尽返末次快照。取到即成功(busy/终态是数据不是错误)。
 */
async function fetchStatusSnap({ flags, sid, mcpCall, base, verbose, verboseLog, nowFn }) {
  const wait = flags.wait;
  const deadlineMs = nowFn() + (wait != null ? wait * 1000 : 0);
  let snap = null;
  while (true) {
    const args = { session_id: sid };
    let timeoutMs;
    if (wait != null) {
      const s = waitSlice(deadlineMs, nowFn());
      args.wait_seconds = s;
      timeoutMs = s * 1000 + 15_000; // design §2:wait 场放宽 HTTP 超时
    } else {
      if (flags.detail) args.detail = true;
      timeoutMs = undefined;
    }
    snap = await mcpCall({ base, tool: 'discussion_status', args, timeoutMs, verbose, verboseLog });
    if (snap.busy === false && snap.stop_reason != null) break; // 终态短路先于窗口循环
    if (snap.wait_timed_out !== true) break; // 变化即返(键仅超时才写;无 wait 恒走此出口)
    if (wait == null || nowFn() >= deadlineMs) break; // 窗口尽,返末次快照
  }
  return snap;
}

async function emitVerbPayload(payload, flags, io) {
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine(payload)}\n`);
  } else {
    io.stdout.write(`${ensureTrailingNewline(formatPayloadText(payload))}`);
  }
}

/**
 * discuss 主流程。opts: { base, flags, positionals, io, mcpCall, nowFn }
 * mcpCall/nowFn 注入便于单测(不打真 daemon)。返回退出码。
 */
export async function runDiscuss({ base, flags, positionals, io, mcpCall = callMcpTool, nowFn = Date.now }) {
  const verbose = !!flags.verbose;
  const quiet = !!flags.quiet;
  const verboseLog = (...parts) => {
    if (verbose && !quiet) io.stderr.write(`${parts.join(' ')}\n`);
  };
  const hint = (...parts) => {
    if (!quiet) io.stderr.write(`${parts.join(' ')}\n`);
  };
  const rest = [...positionals];
  const verb = VERBS.includes(rest[0]) ? rest.shift() : null;

  if (verb === 'start') {
    const started = await mcpCall({
      base, tool: 'start_discussion', args: startArgs(flags, topicOf(rest)), verbose, verboseLog,
    });
    if (flags.output === 'json') {
      io.stdout.write(`${toJsonLine({ session_id: started.session_id, request_id: started.request_id })}\n`);
    } else {
      io.stdout.write(`${started.session_id}\n${started.request_id}\n`);
    }
    hint(
      `session ${started.session_id}(只建群不等待;续观察:evl discuss status ${started.session_id} --wait ${flags.timeout};收结果:evl discuss result ${started.session_id})`
    );
    return EXIT.ok;
  }

  if (verb === 'status') {
    const sid = sidOf(rest, 'status');
    const snap = await fetchStatusSnap({ flags, sid, mcpCall, base, verbose, verboseLog, nowFn });
    if (flags.output === 'json') io.stdout.write(`${toJsonLine(snap)}\n`);
    else io.stdout.write(`${ensureTrailingNewline(formatStatusText(snap))}`);
    return EXIT.ok;
  }

  if (verb === 'result') {
    const sid = sidOf(rest, 'result');
    const payload = await mcpCall({
      base, tool: 'discussion_result', args: { session_id: sid }, verbose, verboseLog,
    });
    // 运行中 → daemon 语义错(isError:"still running")已按 EvlError 抛,退 1;
    // 成功取到恒 0——stop_reason 是数据,LLM 读 json 判读(双口径,design §8)
    if (flags.output === 'json') {
      io.stdout.write(`${toJsonLine({ session_id: sid, ...payload })}\n`);
    } else {
      io.stdout.write(`${ensureTrailingNewline(formatResultText(payload))}`);
    }
    return EXIT.ok;
  }

  if (verb === 'cancel' || verb === 'interrupt') {
    const sid = sidOf(rest, verb);
    const tool = verb === 'cancel' ? 'cancel_discussion' : 'interrupt_discussion';
    const payload = await mcpCall({ base, tool, args: { session_id: sid }, verbose, verboseLog });
    await emitVerbPayload(payload, flags, io);
    return EXIT.ok;
  }

  if (verb === 'inject') {
    const sid = sidOf(rest, 'inject');
    const text = rest.slice(1).join(' ').trim();
    if (text === '') {
      throw new UsageError('discuss inject 缺注入文本(用法:evl discuss inject <sid> "<text>")');
    }
    const payload = await mcpCall({
      base, tool: 'inject_message', args: { session_id: sid, text }, verbose, verboseLog,
    });
    await emitVerbPayload(payload, flags, io);
    return EXIT.ok;
  }

  if (verb === 'presets') {
    const payload = await mcpCall({ base, tool: 'list_presets', args: {}, verbose, verboseLog });
    if (flags.output === 'json') {
      io.stdout.write(`${toJsonLine(payload)}\n`);
      return EXIT.ok;
    }
    const presets = payload?.presets ?? [];
    if (presets.length === 0) {
      io.stdout.write('(空)\n');
      return EXIT.ok;
    }
    io.stdout.write(
      `${formatTable(
        presets,
        [
          { header: 'key', key: 'key' },
          { header: 'source', key: 'source' },
          { header: 'moderator', key: 'moderator' },
          { header: 'participants', getValue: (p) => String(p.participants?.length ?? 0) },
          { header: 'name', key: 'name' },
        ]
      )}\n`
    );
    return EXIT.ok;
  }

  // ── 全链(主入口):start → 轮询 → result ──────────────────────────────
  return runFullChain({ base, flags, topic: topicOf(rest), io, mcpCall, nowFn, verbose, verboseLog, hint });
}

/** 全链:建群 → 有界长轮询(wait_seconds ≤ 25)→ discussion_result → 退出码翻译。 */
async function runFullChain({ base, flags, topic, io, mcpCall, nowFn, verbose, verboseLog, hint }) {
  const started = await mcpCall({
    base, tool: 'start_discussion', args: startArgs(flags, topic), verbose, verboseLog,
  });
  const sid = started.session_id;
  hint(
    `session ${sid}(恢复锚点:evl discuss status ${sid} --wait ${flags.timeout} / evl discuss result ${sid})`
  );

  // SIGINT:首次 cancel_discussion(10s 超时,发出即算)退 3;二次立即硬退。
  // session 保留(daemon 语义),与 chat/M1 一致。
  let sigintCount = 0;
  let sigintResolve = null;
  const sigintPromise = new Promise((r) => {
    sigintResolve = r;
  });
  const onSigint = () => {
    sigintCount += 1;
    if (sigintCount === 1) {
      io.stderr.write('\n^C cancel_discussion 发送中(再次 Ctrl-C 立即退;session 保留)…\n');
      sigintResolve(true);
    } else {
      io.stderr.write(`\n硬退(cancel 可能未送达;session ${sid} 保留,兜底:evl discuss status ${sid})\n`);
      process.exit(EXIT.cancelled);
    }
  };
  process.on('SIGINT', onSigint);

  // 轮询中的信号竞争:输家 promise 的 rejection 不落 unhandled
  const raceSigint = (p) => {
    p.catch(() => {});
    return Promise.race([p, sigintPromise]);
  };

  const deadlineMs = nowFn() + flags.timeout * 1000;
  let prev = null;
  try {
    while (true) {
      if (nowFn() >= deadlineMs) return timeoutExit({ flags, sid, io, hint });
      let snap;
      try {
        const s = waitSlice(deadlineMs, nowFn());
        snap = await raceSigint(
          mcpCall({
            base,
            tool: 'discussion_status',
            args: { session_id: sid, wait_seconds: s },
            timeoutMs: s * 1000 + 15_000,
            verbose,
            verboseLog,
          })
        );
      } catch (e) {
        // 轮询中途传输错:不重试(session 在 daemon 继续,恢复哲学与超时同构;
        // 评审未决项 #1 处置),续窗提示随错误文案出去
        throw new EvlError(
          `${e.message};讨论仍在 daemon 侧继续,勿重跑——续窗:evl discuss status ${sid} --wait ${flags.timeout}`
        );
      }
      if (snap === true) break; // SIGINT 赢出竞争
      if (snap.busy === false && snap.stop_reason != null) break; // 终态短路先于一切(结果由 result 报,不打进度行)
      const line = diffProgressLine(prev, snap);
      if (line != null) io.stderr.write(`${line}\n`);
      prev = snap;
      // 变化(wait_timed_out !== true)与 slice 超时(=== true)都续轮;
      // 出口只有终态 / deadline / 信号
    }

    if (sigintCount > 0) {
      try {
        await mcpCall({
          base, tool: 'cancel_discussion', args: { session_id: sid }, timeoutMs: 10_000, verbose, verboseLog,
        });
      } catch (e) {
        io.stderr.write(`evl: warn: cancel_discussion 失败:${e.message}(session 保留;兜底:evl discuss status ${sid})\n`);
      }
      hint(`已发 cancel_discussion(session ${sid} 保留;续观察:evl discuss status ${sid} --wait ${flags.timeout})`);
      return EXIT.cancelled;
    }

    const payload = await mcpCall({
      base, tool: 'discussion_result', args: { session_id: sid }, verbose, verboseLog,
    });
    if (flags.output === 'json') {
      // result 载荷无 session_id 键(mcp.rs:1138 面),json 自补(design §4)
      io.stdout.write(`${toJsonLine({ session_id: sid, ...payload })}\n`);
    } else {
      io.stdout.write(`${ensureTrailingNewline(formatResultText(payload))}`);
    }
    const note = stopReasonNote(payload.stop_reason);
    if (note != null) io.stderr.write(`evl: ${note}\n`);
    return stopReasonExitCode(payload.stop_reason);
  } finally {
    process.off('SIGINT', onSigint);
  }
}

/** --timeout 到点:不 cancel(窗口属性 ≠ 工作属性,design §8);防重跑三处
 * 文案之一(stderr 首行);json 载荷带 recovery 续窗命令。退出 7。 */
function timeoutExit({ flags, sid, io, hint }) {
  const recovery = `evl discuss status ${sid} --wait ${flags.timeout}`;
  hint(
    `evl: timeout:${flags.timeout}s 到点,讨论仍在跑,勿重跑(重跑会双花 token)。` +
      `续窗:${recovery};收结果:evl discuss result ${sid}`
  );
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine({ session_id: sid, stop_reason: null, error: 'timeout', recovery })}\n`);
  }
  return EXIT.timeout;
}
