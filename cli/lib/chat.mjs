// lib/chat.mjs — `evl chat` 编排(design §4 双模时序,LLM 主场景 = 非 TTY)。
//
// 纯函数(node --test 覆盖):pickProjectByPath / normalizeUsage /
// makeChatResult / terminalExitCode / mapDecisionKey / summarizeToolInput。
// IO(runChat):project/session 解析 → mode → SSE 先挂 → agent/chat →
// request_id 过滤消费 → 终态(退出码 design §6)。
//
// 关键语义(全部实读源码,见 research/daemon-api-wire-notes.md):
// - SSE 订阅必须先于 agent/chat(RULE-SMOKE-001);终态 = request_id 匹配的
//   kind=done|error,不能拿别的提前判。
// - registry 全局广播:按 request_id + session_id 过滤。
// - permission:ask payload 是 **camelCase**(sessionId/toolName/toolInput/rid);
//   chat-event payload 是 snake_case(kind/request_id/stop_reason)——别搞混。
// - 挂 SSE 即 live observer:ask 会等 120s,所以每个本 session 的 ask 必须
//   应答;非交互立即主动 deny(毫秒级,不等 GC3 快拒)。
import process from 'node:process';
import path from 'node:path';
import crypto from 'node:crypto';
import { api, EvlError, EXIT } from './api.mjs';
import { connectSse } from './sse.mjs';
import { UsageError, validateMode, defaultModeFor } from './args.mjs';
import { ensureTrailingNewline, truncate } from './format.mjs';

// ── 纯函数 ─────────────────────────────────────────────────────────────

/** 按 path 匹配 project(turn-smoke.sh:120 同款:词法规整后比对,不解析符号链接)。 */
export function pickProjectByPath(projects, wantPath) {
  if (!wantPath) return undefined;
  const want = path.resolve(wantPath);
  return (projects ?? []).find((p) => p.path && path.resolve(p.path) === want);
}

/** usage 归一:TokenUsage 五字段(int)+ context_window(turn_usage 顶层)。
 * 缺字段补 0;无 usage(null/cancel/error)→ null。 */
export function normalizeUsage(usage, contextWindow = null) {
  if (!usage || typeof usage !== 'object') return null;
  const num = (v) => (Number.isFinite(Number(v)) ? Number(v) : 0);
  return {
    input_tokens: num(usage.input_tokens),
    output_tokens: num(usage.output_tokens),
    cache_creation_input_tokens: num(usage.cache_creation_input_tokens),
    cache_read_input_tokens: num(usage.cache_read_input_tokens),
    context_input_tokens: num(usage.context_input_tokens),
    context_window: contextWindow == null ? null : num(contextWindow),
  };
}

/**
 * json 终态(design §7.5 **恒定形状**,成功/失败同一对象,LLM 解析器一条分支):
 * {text, usage, session_id, request_id, stop_reason, permission_denials, text_chars, error?}
 * error 分支 text="" + text_chars=0 + error:{kind,message};done 分支无 error 键。
 */
export function makeChatResult({
  kind,
  text = '',
  usage = null,
  sessionId,
  requestId,
  stopReason = null,
  denials = 0,
  errorKind = null,
  errorMessage = null,
}) {
  const isError = kind !== 'done';
  const result = {
    text: isError ? '' : text,
    usage,
    session_id: sessionId,
    request_id: requestId,
    stop_reason: stopReason,
    permission_denials: denials,
    text_chars: isError ? 0 : text.length,
  };
  if (isError) {
    result.error = { kind: errorKind ?? kind, message: errorMessage ?? '' };
  }
  return result;
}

/** 终态 → 退出码(design §6):done 0 / error 2 / cancelled 3 / timeout 7;
 * stream_lost(SSE 断)= 脚本错 1。 */
export function terminalExitCode(kind) {
  const table = { done: EXIT.ok, error: EXIT.chatError, cancelled: EXIT.cancelled, timeout: EXIT.timeout };
  return table[kind] ?? EXIT.scriptError;
}

/** TTY 权限应答键位:y=allow_once / a=allow_always / 其他(含 EOF/超时)=deny。 */
export function mapDecisionKey(ch) {
  if (ch === 'y') return 'allow_once';
  if (ch === 'a') return 'allow_always';
  return 'deny';
}

/** toolInput 摘要(stderr 人向/交互提示用,不进 stdout)。 */
export function summarizeToolInput(input, max = 160) {
  let s;
  if (typeof input === 'string') s = input;
  else {
    try {
      s = JSON.stringify(input);
    } catch {
      s = '';
    }
  }
  return truncate(s ?? '', max);
}

function makeRequestId() {
  return `evl-${Date.now().toString(36)}-${crypto.randomBytes(4).toString('hex')}`;
}

// ── IO 编排 ────────────────────────────────────────────────────────────

async function resolveProjectId(base, { projectPath, verbose, verboseLog }) {
  // 列表带 filter:{hidden:true} 查全量(group-chat-run 同款;hidden 项目
  // 不在默认列表,但 create_project 唯一性检查查全表)
  const list = await api(base, 'projects/list_projects', {
    body: { filter: { hidden: true } },
    verbose,
    verboseLog,
  });
  const want = projectPath || process.cwd();
  const hit = pickProjectByPath(list, want);
  if (hit) return hit.id;
  const created = await api(base, 'projects/create_project', {
    body: { path: want },
    verbose,
    verboseLog,
  });
  return created.id;
}

/** TTY 单键应答;EOF/120s 无输入 → null(= deny,daemon ask 窗口同为 120s)。 */
function readDecisionKey(stdin, timeoutMs = 120_000) {
  return new Promise((resolve) => {
    if (!stdin || (!stdin.isTTY && stdin.readable !== true)) {
      resolve(null);
      return;
    }
    let settled = false;
    const wasRaw = stdin.isRaw;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      stdin.removeListener('data', onData);
      stdin.removeListener('end', onEnd);
      stdin.removeListener('error', onEnd);
      try {
        if (stdin.isTTY) stdin.setRawMode(wasRaw ?? false);
      } catch {
        // 非 TTY 时 setRawMode 会抛,忽略
      }
      stdin.pause();
      resolve(value);
    };
    const onData = (buf) => {
      const ch = buf.toString('utf8').trim().toLowerCase()[0] ?? null;
      finish(ch);
    };
    const onEnd = () => finish(null);
    const timer = setTimeout(() => finish(null), timeoutMs);
    timer.unref?.();
    try {
      if (stdin.isTTY) stdin.setRawMode(true);
    } catch {
      // 同上
    }
    stdin.resume();
    stdin.once('data', onData);
    stdin.once('end', onEnd);
    stdin.once('error', onEnd);
  });
}

async function askInteractive(ask, io) {
  io.stderr.write(
    `\npermission ask: ${ask.toolName} ${summarizeToolInput(ask.toolInput)}\n` +
      `[y] allow_once  [a] allow_always  [n] deny > `
  );
  const ch = await readDecisionKey(io.stdin);
  io.stderr.write(`${ch ?? '(eof/timeout → deny)'}\n`);
  return mapDecisionKey(ch);
}

/**
 * chat 主流程。opts: { base, flags, message, io }
 * io: { stdout, stderr, stdin }(注入便于测试与复用)。
 * 返回退出码;stdout 只出数据(design §7.5)。
 */
export async function runChat({ base, flags, message, io }) {
  const verbose = !!flags.verbose;
  const quiet = !!flags.quiet;
  const verboseLog = (...parts) => {
    if (verbose && !quiet) io.stderr.write(`${parts.join(' ')}\n`);
  };
  const hint = (...parts) => {
    if (!quiet) io.stderr.write(`${parts.join(' ')}\n`);
  };
  const interactive = !!io.stdout.isTTY && !flags.nonInteractive;
  // 流式渲染只在:TTY + text 输出 + 交互模式(json 输出下流式会污染 stdout 契约)
  const streamToStdout = interactive && flags.output !== 'json';

  // 0. --mode 值域 CLI 侧校验(在任何 API 调用前;daemon lenient 回退 edit,CLI 必拦)
  const explicitMode = flags.mode != null ? validateMode(flags.mode) : null;

  // 1-2. session(与 project)解析
  const givenSessionId = flags.session || null;
  if (flags.ephemeral && givenSessionId) {
    throw new UsageError('--ephemeral 与 --session 互斥(--ephemeral 只用于本次新建并即弃的 session)');
  }
  let sessionId = givenSessionId;
  if (sessionId == null) {
    const projectId = await resolveProjectId(base, {
      projectPath: flags.project,
      verbose,
      verboseLog,
    });
    const initialCwd = flags.project || process.cwd();
    const row = await api(base, 'sessions/create_session', {
      body: { project_id: projectId, initial_cwd: initialCwd, model: flags.model ?? undefined },
      verbose,
      verboseLog,
    });
    sessionId = row.id;
  } else if (flags.model != null) {
    hint('note: --model 只作用于新建 session;续聊沿用 session 既有 model');
  }

  // mode:set_session_mode 是持久覆盖(写 sessions.mode + audit,design §5)。
  // 显式 --mode → 恒 set;未显式 → TTY 默认 edit(不动),非 TTY 默认 plan(set)。
  const modeToSet = explicitMode ?? (interactive ? null : defaultModeFor(false));
  if (modeToSet) {
    await api(base, 'permissions/set_session_mode', {
      body: { session_id: sessionId, mode: modeToSet },
      verbose,
      verboseLog,
    });
    hint(`mode: ${modeToSet}${givenSessionId ? ' (saved to session, persistent)' : ''}`);
  } else {
    hint('mode: edit (session 默认,未显式设置)');
  }

  // 3. SSE 先挂(RULE-SMOKE-001;connectSse 立即连接,非惰性)
  const ac = new AbortController();
  const stream = await connectSse(base, { signal: ac.signal });

  // 超时/SIGINT → cancel_chat(request_id 域硬停,session 保留);cancel 后给
  // 终态 10s 宽限,还不来就断流按 timedOut/sigint 收尾
  const requestId = makeRequestId();
  let cancelSent = false;
  const sendCancel = async () => {
    if (cancelSent) return;
    cancelSent = true;
    try {
      await api(base, 'cancel/cancel_chat', {
        body: { request_id: requestId },
        timeoutMs: 10_000,
        verbose,
        verboseLog,
      });
    } catch (e) {
      io.stderr.write(`evl: warn: cancel_chat 失败:${e.message}\n`);
    }
    const grace = setTimeout(() => ac.abort(), 10_000);
    grace.unref?.();
  };

  let timedOut = false;
  let sigintCount = 0;
  const timeoutTimer = setTimeout(() => {
    timedOut = true;
    void sendCancel();
  }, flags.timeout * 1000);
  timeoutTimer.unref?.();

  const onSigint = () => {
    sigintCount += 1;
    if (sigintCount === 1) {
      io.stderr.write('\n^C cancel 中(再次 Ctrl-C 立即退出;session 保留)…\n');
      void sendCancel();
    } else {
      io.stderr.write('\n硬退(cancel 请求可能仍在跑;evl sessions 查 busy / GUI Stop 兜底)\n');
      process.exit(EXIT.cancelled);
    }
  };
  process.on('SIGINT', onSigint);

  // 消费状态
  let text = '';
  let denials = 0;
  let lastUsage = null;
  let terminal = null; // {kind, stopReason?, usage?, errorKind?, errorMessage?}
  let accepted = false; // agent/chat 已受理(started)
  let midStreamError = null; // 受理后、终态前的异常(SSE 崩等)

  try {
    // 4. POST agent/chat(fire-and-forget;acceptance.status != started → 报错)
    const acceptance = await api(base, 'agent/chat', {
      body: {
        request_id: requestId,
        session_id: sessionId,
        messages: [{ role: 'user', content: message }],
      },
      verbose,
      verboseLog,
    });
    if (acceptance?.status !== 'started') {
      throw new EvlError(
        `chat 未受理(status=${JSON.stringify(acceptance?.status ?? null)});` +
          'busy 场景无流:classic 会话排队(queued)与群聊注入(injected)CLI 均不支持等待' +
          '(排队消息会在 daemon 侧照跑,排查见 evl sessions 的 busy)'
      );
    }
    accepted = true;

    // 5. 消费循环
    for await (const ev of stream) {
      const name = ev.event;
      const d = ev.data;
      verboseLog(`sse ${name}`);

      if (name === 'chat-event') {
        if (d?.request_id !== requestId) continue; // registry 全局广播,按 rid 过滤
        switch (d.kind) {
          case 'start':
            // run 内每次 LLM 调用的边界:轮与轮之间补分隔(首轮 text 为空不加)
            if (text !== '') {
              text += '\n\n';
              if (streamToStdout) io.stdout.write('\n\n');
            }
            break;
          case 'delta':
            text += d.text ?? '';
            if (streamToStdout) io.stdout.write(d.text ?? '');
            break;
          case 'thinking_delta':
            verboseLog(`thinking: ${truncate(d.text, 120)}`);
            break;
          case 'retrying':
            hint(`retrying ${d.attempt}/${d.max_attempts} wait ${d.wait_ms}ms: ${truncate(d.reason, 160)}`);
            break;
          case 'turn_usage':
            lastUsage = normalizeUsage(d.usage, d.context_window);
            break;
          case 'done':
            terminal = {
              kind: timedOut ? 'timeout' : sigintCount > 0 ? 'cancelled' : 'done',
              stopReason: d.stop_reason ?? null,
              usage: lastUsage ?? normalizeUsage(d.usage),
            };
            break;
          case 'error':
            terminal = {
              kind: 'error',
              stopReason: null,
              usage: lastUsage,
              errorKind: d.category ?? 'error',
              errorMessage: d.message ?? 'unknown chat error',
            };
            break;
          default:
            verboseLog(`chat-event kind=${d.kind} 忽略`);
        }
      } else if (name === 'tool:call' || name === 'tool:result') {
        if (d?.request_id !== requestId) continue;
        // wire 形状不同(state.rs):ToolCallPayload {name, input} vs
        // ToolResultPayload {tool_use_id, content, is_error}——各按字段读。
        if (name === 'tool:call') {
          verboseLog(`tool:call ${d?.name ?? ''} ${summarizeToolInput(d?.input, 120)}`);
        } else {
          verboseLog(
            `tool:result ${d?.tool_use_id ?? ''}${d?.is_error ? ' (error)' : ''} ` +
              summarizeToolInput(d?.content, 120)
          );
        }
      } else if (name === 'permission:ask') {
        // camelCase payload:rid/sessionId/toolName/toolInput
        if (d?.sessionId !== sessionId) {
          verboseLog(`permission:ask 他 session(${d?.sessionId})忽略`);
          continue;
        }
        const decision = interactive ? await askInteractive(d, io) : 'deny';
        if (decision === 'deny') denials += 1;
        try {
          await api(base, 'permissions/permission_response', {
            body: {
              rid: d.rid,
              decision,
              reason: decision === 'deny' ? 'evl: denied (non-interactive auto-deny or user n)' : undefined,
            },
            timeoutMs: 10_000,
            verbose,
            verboseLog,
          });
        } catch (e) {
          // 应答失败不终结消费:daemon 侧 ask 无人应答会自行超时快拒,
          // turn 继续跑 —— 这里丢掉终态跟踪反而制造 busy 残留
          io.stderr.write(`evl: warn: permission_response 失败:${e.message}\n`);
        }
      } else if (name === 'tool:question' || name === 'mode:change:request') {
        // 两模一律 stderr 提示 + 忽略(daemon 侧超时自理;prd 未决 #4)
        hint(`note: ${name} 到达,CLI 忽略(daemon 侧超时自理)`);
      }
      // 其他事件名(subagent:event 等)不在 CLI 关心面,静默
      if (terminal) break;
    }
  } catch (e) {
    if (e?.name !== 'AbortError') {
      if (!accepted) {
        // 受理前失败(建 session/mode/POST 本身):turn 未启动,干净地按脚本错抛
        clearTimeout(timeoutTimer);
        process.off('SIGINT', onSigint);
        ac.abort();
        throw e;
      }
      // 受理后、终态前的异常(流崩/读失败):按 stream_lost 收尾而不是裸抛 ——
      // 保 json 恒定形状 + session 提示/--ephemeral 清理照走(design §7.5/§8)
      midStreamError = e;
    }
  }
  clearTimeout(timeoutTimer);
  process.off('SIGINT', onSigint);
  ac.abort();

  // 流断在终态前:按 timedOut > sigint > stream_lost 定终态
  if (!terminal) {
    if (timedOut) {
      terminal = {
        kind: 'timeout',
        stopReason: null,
        usage: lastUsage,
        errorKind: 'timeout',
        errorMessage: `no terminal event within ${flags.timeout}s (cancel_chat sent)`,
      };
    } else if (sigintCount > 0) {
      terminal = {
        kind: 'cancelled',
        stopReason: null,
        usage: lastUsage,
        errorKind: 'cancelled',
        errorMessage: 'interrupted by SIGINT (cancel_chat sent, session kept)',
      };
    } else {
      terminal = {
        kind: 'stream_lost',
        stopReason: null,
        usage: lastUsage,
        errorKind: 'stream_lost',
        errorMessage: midStreamError
          ? `SSE 流在终态前异常断开(${midStreamError?.message ?? midStreamError};` +
            'design §8:单发生命周期短,断 = 报错由调用方重试)'
          : 'SSE 流在终态前断开(design §8:单发生命周期短,断 = 报错由调用方重试)',
      };
    }
  }

  // 6. 收尾:--ephemeral 删 session;正常 stderr 打 session id(续聊提示)
  if (flags.ephemeral) {
    try {
      await api(base, 'sessions/delete_session', {
        body: { session_id: sessionId },
        verbose,
        verboseLog,
      });
      hint(`session ${sessionId} 已删(--ephemeral)`);
    } catch (e) {
      io.stderr.write(`evl: warn: delete_session 失败:${e.message}\n`);
    }
  } else {
    hint(`session: ${sessionId} (保留;续聊加 --session ${sessionId};--ephemeral 即弃)`);
  }

  const result = makeChatResult({
    kind: terminal.kind,
    text,
    usage: terminal.usage ?? null,
    sessionId,
    requestId,
    stopReason: terminal.stopReason ?? null,
    denials,
    errorKind: terminal.errorKind,
    errorMessage: terminal.errorMessage,
  });

  // stdout 只出数据(design §7.5)
  if (flags.output === 'json') {
    io.stdout.write(`${JSON.stringify(result)}\n`);
  } else if (terminal.kind === 'done') {
    if (streamToStdout) {
      if (text !== '' && !text.endsWith('\n')) io.stdout.write('\n');
    } else {
      io.stdout.write(ensureTrailingNewline(text));
    }
  } else {
    io.stderr.write(
      `evl: chat ${terminal.kind}: ${result.error?.message ?? ''}(详情: ./scripts/daemon.sh logs)\n`
    );
  }
  return terminalExitCode(terminal.kind);
}
