// group-chat-run 引擎纯函数单测(node:test;shape 断言非快照——评审团
// 2026-09-06 verdict:测试锁结构,不锁全文)。跑法:node --test scripts/。
// 注意:vitest include 只收 app/src,本文件走 node 内建 runner,互不干扰。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import {
  EXIT, PRESETS, composePresets, resolveParticipants, buildCreateSessionBody, buildChatBody,
  normalizeModelRef, validateModelRefs, summarizeToolUses, defaultTranscriptPath,
  renderTranscript, injectGuardDecision, interpretAcceptance,
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
  // 三预设齐 + 名单/主持人模型与历史阵容一致(名字形态,不是 UUID ——
  // 名字→UUID 解析发生在 run 时 normalizeModelRef)。
  assert.deepEqual(Object.keys(PRESETS), ['review', 'arch', 'retro']);
  assert.equal(PRESETS.review.moderator_model, 'MiniMax-M3');
  assert.deepEqual(
    PRESETS.review.participants.map((p) => [p.name, p.model]),
    [['架构', 'glm-5.3'], ['产品', 'GLM-5.3-Flash'], ['后端', 'deepseek-v4-flash']],
  );
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
