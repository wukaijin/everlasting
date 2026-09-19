// lib/commands/status.mjs — `evl status`:daemon health + 版本(GET /api/v1/health)。
// daemon 不可达时 api() 抛 EvlError(含 OS 错误翻译 + ./scripts/daemon.sh bg 提示)。
import { api } from '../api.mjs';
import { toJsonLine } from '../format.mjs';

export async function runStatus({ base, flags, io }) {
  const health = await api(base, 'health', { method: 'GET', verbose: flags.verbose });
  if (flags.output === 'json') {
    io.stdout.write(`${toJsonLine(health)}\n`);
    return 0;
  }
  io.stdout.write(
    [
      `daemon:   ${health.daemonId ?? '?'}`,
      `version:  ${health.daemonVersion ?? '?'}`,
      `api:      ${(health.apiVersions ?? []).join(',') || '?'}`,
      `uptime:   ${health.uptimeSeconds ?? '?'}s`,
      '',
    ].join('\n')
  );
  return 0;
}
