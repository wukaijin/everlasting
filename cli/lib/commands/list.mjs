// lib/commands/list.mjs — 内省四件套(sessions / projects / models / usage)。
// 只读;--output json 单行 JSON 机器可读(AC3),text 人类可读表。
//
// wire 注(design §2 实读):
// - list_projects 带 filter:{hidden:true} 查全量(group-chat-run 同款;hidden
//   项目不在默认列表,但 create_project 唯一性检查查全表)。
// - list_sessions 必填 project_id(routes/sessions.rs ListSessionsRequest)——
//   `evl sessions` 默认遍历全部 project 合并(纯运输层聚合,零编排策略),
//   --project <path> 收窄到单 project。
// - list_models / get_default_model 是 camelCase(ModelRow 无 rename);
//   sessions / projects 是 snake_case——各自按原样透传,不做改名。
import { api, EvlError } from '../api.mjs';
import { formatTable, toJsonLine, truncate } from '../format.mjs';
import { pickProjectByPath } from '../chat.mjs';

function emit(view, flags, io, columns) {
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine(view)}\n`);
    return 0;
  }
  if (view.length === 0) {
    io.stdout.write('(空)\n');
    return 0;
  }
  io.stdout.write(`${formatTable(view, columns)}\n`);
  return 0;
}

async function listProjectsAll(base, flags) {
  return api(base, 'projects/list_projects', {
    body: { filter: { hidden: true } },
    verbose: flags.verbose,
  });
}

export async function runProjects({ base, flags, io }) {
  const rows = await listProjectsAll(base, flags);
  const view = (rows ?? []).map((p) => ({
    id: p.id,
    name: p.name,
    path: p.path,
    hidden: !!p.hidden,
    git_branch: p.git_branch,
    updated_at: p.updated_at,
  }));
  return emit(
    view,
    flags,
    io,
    [
      { header: 'id', key: 'id' },
      { header: 'name', key: 'name' },
      { header: 'path', key: 'path' },
      { header: 'hidden', getValue: (r) => (r.hidden ? 'yes' : '') },
      { header: 'branch', getValue: (r) => r.git_branch ?? '' },
      { header: 'updated_at', key: 'updated_at' },
    ]
  );
}

export async function runSessions({ base, flags, io }) {
  const projects = await listProjectsAll(base, flags);
  let targets = projects ?? [];
  if (flags.project) {
    const hit = pickProjectByPath(targets, flags.project);
    if (!hit) {
      throw new EvlError(`project 不存在:${flags.project}(用 evl projects 查列表)`);
    }
    targets = [hit];
  }
  const all = [];
  for (const p of targets) {
    const rows = await api(base, 'sessions/list_sessions', {
      body: { project_id: p.id },
      verbose: flags.verbose,
    });
    for (const s of rows ?? []) {
      all.push({
        id: s.id,
        title: s.title,
        project_path: p.path,
        session_type: s.session_type,
        updated_at: s.updated_at,
        busy: !!s.busy,
        stop_reason: s.stop_reason ?? null,
      });
    }
  }
  all.sort((a, b) => String(b.updated_at ?? '').localeCompare(String(a.updated_at ?? '')));
  return emit(
    all,
    flags,
    io,
    [
      { header: 'id', key: 'id' },
      { header: 'busy', getValue: (r) => (r.busy ? 'busy' : '') },
      { header: 'stop_reason', getValue: (r) => r.stop_reason ?? '' },
      { header: 'updated_at', key: 'updated_at' },
      { header: 'type', key: 'session_type' },
      { header: 'title', getValue: (r) => truncate(r.title, 48) },
    ]
  );
}

export async function runModels({ base, flags, io }) {
  const models = (await api(base, 'providers/list_models', { body: {}, verbose: flags.verbose })) ?? [];
  const def = await api(base, 'providers/get_default_model', { body: {}, verbose: flags.verbose });
  const defaultId = def?.id ?? null;
  const view = models.map((m) => ({
    id: m.id,
    display_name: m.displayName,
    provider: m.providerDisplayName,
    protocol: m.providerProtocol,
    disabled: !!m.disabled || !!m.providerDisabled,
    is_default: m.id === defaultId,
  }));
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine({ default: defaultId, models: view })}\n`);
    return 0;
  }
  if (view.length === 0) {
    io.stdout.write('(空)\n');
    return 0;
  }
  io.stdout.write(
    `${formatTable(
      view,
      [
        { header: 'id', key: 'id' },
        { header: 'default', getValue: (r) => (r.is_default ? '*' : '') },
        { header: 'display_name', key: 'display_name' },
        { header: 'provider', key: 'provider' },
        { header: 'protocol', key: 'protocol' },
        { header: 'disabled', getValue: (r) => (r.disabled ? 'yes' : '') },
      ]
    )}\n`
  );
  return 0;
}

export async function runUsage({ base, flags, io }) {
  const report = await api(base, 'usage/usage_window', {
    body: { provider_id: flags.provider ?? null },
    verbose: flags.verbose,
  });
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine(report)}\n`);
    return 0;
  }
  const providers = report?.providers ?? [];
  const lines = [];
  lines.push(`window: ${report?.windowHours ?? '?'}h`);
  if (providers.length === 0) {
    lines.push('(窗口内无用量)');
  } else {
    lines.push(
      formatTable(
        providers,
        [
          { header: 'provider', key: 'displayName' },
          { header: 'input', getValue: (p) => p.totals?.inputTokens ?? 0 },
          { header: 'output', getValue: (p) => p.totals?.outputTokens ?? 0 },
          { header: 'cache_read', getValue: (p) => p.totals?.cacheReadInputTokens ?? 0 },
          { header: 'cache_creation', getValue: (p) => p.totals?.cacheCreationInputTokens ?? 0 },
        ]
      )
    );
  }
  const top = report?.topSessions ?? [];
  if (top.length > 0) {
    lines.push('');
    lines.push(
      formatTable(
        top,
        [
          { header: 'session', getValue: (s) => String(s.sessionId ?? '').slice(0, 8) },
          { header: 'window_input', getValue: (s) => s.windowMainInput ?? 0 },
          { header: 'title', getValue: (s) => truncate(s.title, 48) },
        ]
      )
    );
  }
  io.stdout.write(`${lines.join('\n')}\n`);
  return 0;
}
