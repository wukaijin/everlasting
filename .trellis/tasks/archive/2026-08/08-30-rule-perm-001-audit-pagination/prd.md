# PRD — RULE-PERM-001 审计事件查询分页

> 债源:`.trellis/reviews/DEBT.md` §RULE-PERM-001(P3,Permission);发现于 2026-08-27 技术债盘点。
> 原始 finding:C4 审计事件查询 MVP 全量拉取,无分页无虚拟滚动;PRD Edge Cases 标 TODO「>500 条事件的 session」;索引让 ORDER BY 够快故暂无实测投诉。

## 背景与问题

`list_session_audit_events`(Tauri + daemon 双 transport)对 `session_audit_events`
按 session **全量拉取**(`db/permissions.rs:298` 无 LIMIT)。两个消费方:

1. **AuditLogModal**(`stores/audit.ts` + `components/audit/AuditLogModal.vue`):
   全量行驻内存,类别过滤 / 仅 critical 均为客户端过滤,计数 chip
   (「X / Y 项」)派生自全量数组。长会话(高频工具轮,每 tool call 1–2 行
   审计)行数会到千级:单次 IPC 载荷、列表首屏渲染、内存驻留都随总行数线性涨。
2. **traceStore**(`stores/traceStore.ts:302`):拉全量审计行按 `turnSeq` 分组挂到
   turn 时间线 —— 数据语义上就需要全部行,本任务**不动**它(见非目标)。

修法方向按 DEBT 登记:LIMIT/OFFSET 或 keyset 分页;design.md 定案 keyset。

## Goal

审计日志弹窗从「全量拉取 + 客户端过滤」改为「keyset 游标分页 + 服务端过滤/计数」,
UI 语义(排序、计数 chip、critical 徽标)与现状**逐项等价**;旧全量命令原样保留。

## Requirements

- R1. 弹窗首屏只拉一页(默认 100 行,新→旧),底部「加载更多」按游标续拉,
  追加不重复不跳行;已到末尾时入口消失。
- R2. 类别过滤 / 仅 critical 下推到 SQL;过滤条件变化从第一页重新加载,
  「加载更多」续拉的行都满足当前过滤(计数与列表口径一致)。
- R3. 计数 chip 语义与现状完全一致:总数(不过滤)、critical 数(不受类别过滤
  影响)、filtered 数(当前过滤命中);数值来自服务端,对未加载的行也准确。
- R4. 排序语义与现状一致:`ts DESC, id DESC`(同秒 tie 由 id 决定,与前端
  现行 `sortEvents` 二级键相同);由 SQL 保证,前端不再重排。
- R5. 分页在「弹窗开着、agent 正在跑、新审计行持续追加」场景下稳定:
  游标定位的「更早页」不因新行插入而重复/跳行(keyset 性质,验收含行为测试)。
- R6. 旧命令 `list_session_audit_events`(全量)wire 行为不变,traceStore
  零改动;新能力走**新增命令**,双 transport(Tauri + daemon HTTP)同步落地。
- R7. `payload_json` 畸形 JSON / NULL 的历史行不因 critical 判定(服务端
  json_extract)导致查询报错,行为对齐现行客户端 `isCritical` 的容错(视为非 critical)。

## 约束

- C1. wire 变更 additive:不改既有命令的参数与返回形状;e2e route-mock 清单
  (`app/e2e/fixtures.ts`)与 `all_command_names` 同步登记新命令。
- C2. 虚拟滚动不做(分页后单页 100 行,原生渲染足够);DEBT 闭合以「分页落地」计。
- C3. 页大小固定 100(后端接受 limit 但 cap 500,防误用);不做页码跳转 UI。
- C4. 审计表为 append-only,本任务零 schema 变更、零 migration。

## 非目标

- traceStore / TracePanel 的全量拉取保持不动:turn 分组语义上需要全量行,
  且属调试面板,行数膨胀的优先级低;若未来成为实测问题另立任务。
- 审计事件的关键词搜索 / 时间范围过滤(现无此需求)。
- 移动端专项适配(弹窗已有响应式处理,不在本任务重审)。

## Acceptance Criteria

- [x] AC1. db 层行为测试:同秒 tie 排序 (ts DESC, id DESC);keyset 游标续拉
  在「取页后插入新行」下不重不漏(OFFSET 会重复,行为测试钉死);limit 生效。
- [x] AC2. db 层过滤测试:kind 过滤、critical 过滤(含 critical true / false /
  payload NULL / payload 畸形 JSON 四态)命中正确,畸形 JSON 不报错。
- [x] AC3. 计数测试:matched / totalAll / totalCritical 三值与种子行精确相等,
  过滤组合下 matched 正确。
- [x] AC4. 双 transport 同步:Tauri command + daemon route 注册 + http.ts 映射 +
  `all_command_names` + e2e fixtures 清单五处齐;e2e 全量绿(catch-all 500 不误伤)。
- [x] AC5. 前端 store 测试:首屏 reset+加载、loadMore 游标追加、过滤变更重拉、
  hasMore 终止、计数映射、错误路径保旧值;AuditLogModal 组件测试覆盖
  「加载更多」按钮可见性随 hasMore 变化。
- [x] AC6. 手动冒烟:长会话(>100 审计行,可用既有 dev DB)打开弹窗首屏 100 行,
  连点加载更多到末尾,计数 chip 数值与 sqlite 只读查询一致;过滤切换重拉正常。
- [x] AC7. 全量 `cargo test -p everlasting --lib` 绿 + clippy `-D warnings` + fmt;
  `pnpm test` + vue-tsc 绿。
- [x] AC8. DEBT.md 删除 §RULE-PERM-001(闭合走 git log 追溯),优先级分布表归零。
