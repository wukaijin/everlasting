// group-chat-run 引擎纯函数单测(node:test;shape 断言非快照——评审团
// 2026-09-06 verdict:测试锁结构,不锁全文)。跑法:node --test scripts/。
// 注意:vitest include 只收 app/src,本文件走 node 内建 runner,互不干扰。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import {
  EXIT, PRESETS, composePresets, resolveParticipants, buildCreateSessionBody, buildChatBody,
  aggregateTokens,
  normalizeModelRef, validateModelRefs, summarizeToolUses, defaultTranscriptPath,
  renderTranscript, renderConclusionsSection, injectGuardDecision, interpretAcceptance,
} from './group-chat-run.mjs';
import presetsFile from './group-chat-presets.json' with { type: 'json' };

const MODELS = [
  { id: 'uuid-glm53', modelName: 'glm-5.3', displayName: 'glm-5.3' },
  { id: 'uuid-flash', modelName: 'glm-5.3-flash', displayName: 'GLM-5.3-Flash' },
  { id: 'uuid-m3', modelName: 'MiniMax-M3', displayName: 'MiniMax-M3' },
];

test('resolveParticipants:预设默认 + 单人覆盖两级(AC3)', () => {
  const base = resolveParticipants({ preset: 'review', set: [] });
  assert.equal(base.length, 3);
  assert.deepEqual(base.map((p) => p.name), ['架构', '产品', '后端']);

  const over = resolveParticipants({ preset: 'review', set: ['架构.model=deepseek'] });
  assert.equal(over.find((p) => p.name === '架构').model, 'deepseek');
  // 预设不被 --set 原地污染
  assert.equal(PRESETS.review.participants[0].model, 'glm-5.3');

  assert.throws(() => resolveParticipants({ preset: 'review', set: ['不存在.model=x'] }), /不在名单/);
  assert.throws(() => resolveParticipants({ preset: 'nope', set: [] }), /未知预设/);
});

test('resolveParticipants:--participants 整名单替换 = 增删语义超集', () => {
  const list = resolveParticipants({
    participantsJson: JSON.stringify([
      { name: '甲', model: 'glm-5.3' },
      { name: '乙', model: 'MiniMax-M3', persona_md: 'x' },
    ]),
    set: [],
  });
  assert.equal(list.length, 2);
  assert.equal(list[0].persona_md, undefined);
  assert.throws(() => resolveParticipants({ participantsJson: '[]', set: [] }), /非空/);
  assert.throws(() => resolveParticipants({ participantsJson: '[{"name":"甲","model":"a"},{"name":"甲","model":"b"}]', set: [] }), /重名/);
});

test('buildCreateSessionBody / buildChatBody:wire 形状', () => {
  const body = buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3',
    participants: [{ name: '甲', model: 'uuid-glm53' }],
  });
  assert.equal(body.session_type, 'group_chat');
  assert.equal(body.model, 'uuid-m3'); // moderator 走 session.model,必须是 UUID
  assert.deepEqual(body.metadata.participants, [{ name: '甲', model: 'uuid-glm53' }]);

  const chat = buildChatBody({ requestId: 'r1', sessionId: 's1', topic: '议题' });
  assert.deepEqual(chat.messages, [{ role: 'user', content: '议题' }]);
});

test('normalizeModelRef:UUID 直收 / 精确名优先 / 大小写不敏感兜底 / 失配报清单', () => {
  assert.equal(normalizeModelRef(MODELS, 'uuid-glm53'), 'uuid-glm53');
  assert.equal(normalizeModelRef(MODELS, 'GLM-5.3-Flash'), 'uuid-flash'); // displayName 精确命中
  assert.equal(normalizeModelRef(MODELS, 'GLM-5.3'), 'uuid-glm53'); // 大小写变体兜底到 glm-5.3
  assert.equal(normalizeModelRef(MODELS, 'minimax-m3'), 'uuid-m3');
  assert.throws(() => normalizeModelRef(MODELS, 'nope'), /不在目录.*glm-5.3/);
  assert.deepEqual(
    validateModelRefs(MODELS, { moderatorModel: 'MiniMax-M3', participants: [{ name: '甲', model: 'glm-5.3' }] }),
    { moderatorModelId: 'uuid-m3', participants: [{ name: '甲', model: 'uuid-glm53' }] },
  );
});

test('summarizeToolUses:工具轮证据链(name + 关键参数)', () => {
  assert.equal(summarizeToolUses('not-array'), null);
  assert.equal(summarizeToolUses([{ type: 'tool_result', content: 'x' }]), null);
  const s = summarizeToolUses([
    { type: 'tool_use', name: 'grep', input: { pattern: 'group_chat', glob: '*.rs' } },
    { type: 'tool_use', name: 'read_file', input: { file_path: '/a/b.rs' } },
  ]);
  assert.match(s, /grep\{group_chat\}/);
  assert.match(s, /read_file\{\/a\/b\.rs\}/);
});

test('buildCreateSessionBody:createdVia 增量键(有则盖戳,无则不设键 = GUI 语义)', () => {
  const stamped = buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3',
    participants: [], createdVia: 'script',
  });
  assert.equal(stamped.metadata.created_via, 'script');
  const mcp = buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3',
    participants: [], createdVia: 'mcp',
  });
  assert.equal(mcp.metadata.created_via, 'mcp');
  // 不传 = 键不存在(GUI/历史 session 的判定语义,不传空串)
  assert.equal('created_via' in buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3', participants: [],
  }).metadata, false);
});

// gce-m4c(09-08):token_budget 声明键 —— 与 createdVia 同一「有才写」纪律。
test('buildCreateSessionBody:tokenBudget 增量键(声明才写,缺省无键 = 不限)', () => {
  const withBudget = buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3',
    participants: [], tokenBudget: 400000,
  });
  assert.equal(withBudget.metadata.token_budget, 400000);
  const without = buildCreateSessionBody({
    projectId: 'p1', projectPath: '/repo', moderatorModel: 'uuid-m3', participants: [],
  });
  assert.equal('token_budget' in without.metadata, false);
});

// gce-m4c:客户端核算口径 —— 四计费字段求和、context_input 不计、
// worker 行(runId≠'')排除、无 speaker 消息可对齐的行排除、按 speaker 聚合。
test('aggregateTokens:四字段计费口径 + worker/无归属排除 + per_speaker 降序', () => {
  const traces = [
    { seq: 1, runId: '', tokenUsageJson: '{"input_tokens":100,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":100,"context_input_tokens":1000}' },
    { seq: 2, runId: '', tokenUsageJson: '{"input_tokens":200,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":20,"context_input_tokens":200}' },
    // worker 行:同 seq 有 speaker 消息也必须排除
    { seq: 2, runId: 'wrk-1', tokenUsageJson: '{"input_tokens":9999,"output_tokens":9,"cache_creation_input_tokens":0,"cache_read_input_tokens":9,"context_input_tokens":9}' },
    // 无 usage / 坏 JSON / 无消息可对齐:全跳过
    { seq: 3, runId: '', tokenUsageJson: null },
    { seq: 4, runId: '', tokenUsageJson: '{oops' },
    { seq: 9, runId: '', tokenUsageJson: '{"input_tokens":500,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}' },
  ];
  const messages = [
    { seq: 1, role: 'assistant', speaker: 'moderator' },
    { seq: 2, role: 'assistant', speaker: 'Alice' },
    { seq: 3, role: 'assistant', speaker: 'Alice' },
    { seq: 9, role: 'assistant', speaker: null }, // 经典行:对齐不上
    // rewrite 产品行(user 带 speaker):与 daemon SQL 的 role='assistant'
    // 过滤对齐,seq 撞上 trace 行也不计
    { seq: 2, role: 'user', speaker: 'Alice' },
  ];
  const { total, per_speaker } = aggregateTokens(traces, messages);
  // moderator 210 + Alice 240(context_input 都没加进去)
  assert.equal(total, 450);
  assert.deepEqual(per_speaker, [
    { speaker: 'Alice', tokens: 240 },
    { speaker: 'moderator', tokens: 210 },
  ]);
  // 空输入 → 零合计,不抛
  assert.deepEqual(aggregateTokens([], []), { total: 0, per_speaker: [] });
});

test('defaultTranscriptPath:落在 everlasting 仓库根 out/(非 CWD);显式 rootDir 分叉(MCP)', () => {
  const p1 = defaultTranscriptPath('议题 ABC');
  assert.equal(path.dirname(p1), path.resolve(path.dirname(new URL(import.meta.url).pathname), '..', 'out'));
  assert.match(path.basename(p1), /^group-chat-.+-\d{14}\.md$/);
  // MCP 消费:转录落讨论 cwd 的 out/(design §6 有意分叉)
  const p2 = defaultTranscriptPath('议题 ABC', '/work/vue3-cms');
  assert.equal(path.dirname(p2), '/work/vue3-cms/out');
});

test('renderTranscript:blockquote 隔离 / 工具轮证据链 / summary 缺失警告 / 阵容可读名', () => {
  const session = {
    id: 'sid-1', title: '测试', model: 'uuid-m3', metadata: JSON.stringify({ participants: [{ name: '甲', model: 'uuid-glm53' }] }),
    stop_reason: 'group_chat_end', discussion_summary: '共识A', input_tokens_total: 100, output_tokens_total: 5,
  };
  const messages = [
    { seq: 0, role: 'user', speaker: null, text: '议题正文', has_tool_calls: false, has_tool_results: false },
    { seq: 1, role: 'user', speaker: null, text: '', has_tool_calls: false, has_tool_results: true,
      content: [{ type: 'tool_use', name: 'grep', input: { pattern: 'x' } }] },
    { seq: 2, role: 'assistant', speaker: '甲', text: '第一行\n- 列表碎片\n## 标题碎片', has_tool_calls: false, has_tool_results: false },
  ];
  const out = renderTranscript({ session, messages, startedAtMs: 0, stoppedAtMs: 61000, modelNames: { 'uuid-m3': 'MiniMax-M3', 'uuid-glm53': 'glm-5.3' } });
  assert.match(out, /MiniMax-M3 主持 \+ 甲\/glm-5\.3/); // UUID→可读名
  assert.match(out, /discussion_summary\n\n共识A/);
  assert.match(out, /工具调用 grep\{x\}/); // 工具轮证据链
  assert.match(out, /\*\*甲\*\*:\n {2}> 第一行\n {2}> - 列表碎片/); // blockquote 隔离碎格式

  const noSummary = renderTranscript({
    session: { ...session, discussion_summary: null }, messages: [], startedAtMs: 0, stoppedAtMs: 1000,
  });
  assert.match(noSummary, /⚠️ 正常收官但 discussion_summary 缺失/); // 警告落文件
});

test('EXIT 契约:stop_reason 四值各一档,1 留给脚本错误', () => {
  assert.equal(EXIT.groupChatEnd, 0);
  assert.equal(EXIT.maxRounds, 2);
  assert.equal(EXIT.cancelled, 3);
  assert.equal(EXIT.error, 4);
  assert.equal(EXIT.scriptError, 1);
});

test('injectGuardDecision:busy 只信 === true;空闲/已收官一律不发起(评审 P1-1)', () => {
  assert.deepEqual(injectGuardDecision({ busy: true, stop_reason: null }), { allowed: true });
  const blocked = injectGuardDecision({ busy: false, stop_reason: 'group_chat_end' });
  assert.equal(blocked.allowed, false);
  assert.match(blocked.reason, /start_discussion/); // 指引发起新讨论的正确入口
  // additive wire:字段缺失 / null summary 都视为闲(误发会抹旧场 summary,宁拒勿发)
  assert.equal(injectGuardDecision({}).allowed, false);
  assert.equal(injectGuardDecision(null).allowed, false);
});

test('interpretAcceptance:injected 唯一成功态;started/queued/未知 = misfire 带止损动作', () => {
  assert.deepEqual(interpretAcceptance({ status: 'injected' }), { kind: 'injected' });
  assert.deepEqual(
    interpretAcceptance({ status: 'started' }),
    { kind: 'misfire', status: 'started', cancelOwnRequest: true },
  );
  assert.deepEqual(
    interpretAcceptance({ status: 'queued', id: 'q1', position: 2 }),
    { kind: 'misfire', status: 'queued', cancelOwnRequest: true },
  );
  assert.deepEqual(
    interpretAcceptance(null),
    { kind: 'misfire', status: 'unknown', cancelOwnRequest: true },
  );
});

test('PRESETS 单一事实源(M4a R7):来自 group-chat-presets.json,模型用名字,persona 组装含公共纪律', () => {
  // PRESETS 就是 JSON 组装的结果(同输入同输出,锁组装确定性)。
  assert.deepEqual(PRESETS, composePresets(presetsFile));
  // 四预设齐 + 名单/主持人模型与历史阵容一致(名字形态,不是 UUID ——
  // 名字→UUID 解析发生在 run 时 normalizeModelRef)。fe_review = review 的
  // 前端变体(backend→frontend),阵容镜像断言在下方。
  assert.deepEqual(Object.keys(PRESETS), ['review', 'fe_review', 'arch', 'retro']);
  assert.equal(PRESETS.review.moderator_model, 'MiniMax-M3');
  assert.deepEqual(
    PRESETS.review.participants.map((p) => [p.name, p.model]),
    [['架构', 'glm-5.3'], ['产品', 'GLM-5.3-Flash'], ['后端', 'deepseek-v4-flash']],
  );
  assert.deepEqual(
    PRESETS.fe_review.participants.map((p) => [p.name, p.model]),
    [['架构', 'glm-5.3'], ['产品', 'GLM-5.3-Flash'], ['前端', 'deepseek-v4-flash']],
  );
  assert.equal(PRESETS.fe_review.moderator_model, 'MiniMax-M3');
  assert.deepEqual(
    PRESETS.retro.participants.map((p) => [p.name, p.model]),
    [['产品', 'GLM-5.3-Flash'], ['局外', 'glm-5.3']],
  );
  // persona = 视角边界 + "\n\n" + 公共纪律(与旧内置常量同形)。
  for (const preset of Object.values(PRESETS)) {
    for (const p of preset.participants) {
      assert.ok(p.persona_md.endsWith(presetsFile.persona_common), `${p.name} persona 须以公共纪律收尾`);
      assert.ok(p.persona_md.includes('\n\n'), `${p.name} persona 须有边界/纪律分隔`);
    }
  }
  // JSON 形状防御:缺 persona kind / 空 presets → 明确报错。
  assert.throws(() => composePresets({ ...presetsFile, personas: {} }), /缺 persona/);
  assert.throws(() => composePresets({ persona_common: 'x', personas: presetsFile.personas, presets: {} }), /presets 不能为空/);
});

// C2 证据链(09-09-gc-c2-evidence-summary):结构化结论节渲染。
test('renderConclusionsSection:stance 标注 + 锚点校验记号 + 开放问题;坏 JSON/空 detail 省略', () => {
  const detail = JSON.stringify({
    conclusions: [
      { claim: '实锚', anchors: [{ path: 'a.rs', line: 2, check: 'ok' }], stance: 'verified' },
      { claim: '断证', anchors: [{ path: 'b.rs', line: 9, check: 'not_found' }], stance: 'verified' },
      { claim: '推测' }, // stance 缺省 inferred
      { claim: '争议', anchors: [{ path: 'c.rs' }], stance: 'disputed' },
    ],
    open_questions: ['何时复核'],
  });
  const out = renderConclusionsSection(detail);
  assert.match(out, /## conclusions/);
  assert.match(out, /- \[verified\] 实锚 — `a\.rs:2` ✓/);
  assert.match(out, /- \[verified\] 断证 — `b\.rs:9` ⚠\(not_found\)/);
  assert.match(out, /- \[inferred\] 推测\n/);
  assert.match(out, /- \[disputed\] 争议 — `c\.rs`\n/);
  assert.match(out, /## open_questions\n\n- 何时复核/);
  // 降级三臂:坏 JSON / 空结构 / null → 空串(旧场零回归)。
  assert.equal(renderConclusionsSection('{not json'), '');
  assert.equal(renderConclusionsSection('{"conclusions":[],"open_questions":[]}'), '');
  assert.equal(renderConclusionsSection(null), '');
});

test('renderTranscript:session 带 discussion_detail 渲染 conclusions 节;旧场无键省略', () => {
  const base = { id: 'sid-2', title: 't', metadata: null, stop_reason: 'group_chat_end', discussion_summary: 'S' };
  const withDetail = renderTranscript({
    session: { ...base, discussion_detail: JSON.stringify({ conclusions: [{ claim: 'C1', stance: 'verified' }], open_questions: [] }) },
    messages: [], startedAtMs: 0, stoppedAtMs: 1000,
  });
  assert.match(withDetail, /discussion_summary\n\nS/);
  assert.match(withDetail, /## conclusions\n\n- \[verified\] C1/);
  const legacy = renderTranscript({ session: base, messages: [], startedAtMs: 0, stoppedAtMs: 1000 });
  assert.doesNotMatch(legacy, /## conclusions/);
});
