# Design — RULE-PERM-001 审计事件查询分页

## 0. 现状盘点(file:line)

| 位置 | 现状 |
|---|---|
| `app/src-tauri/src/db/permissions.rs:298` | `list_audit_events` 全量 SELECT,`ORDER BY ts DESC`(无 id tie-break,无 LIMIT) |
| `app/src-tauri/src/db/permissions.rs:346` | `AuditEventRow`(camelCase wire,`payload_json` 留 String) |
| `app/src-tauri/src/commands/permissions.rs:423/433` | `_inner` + Tauri command `list_session_audit_events` |
| `app/src-tauri/src/daemon/routes/permissions.rs:101/187` | daemon POST route + 注册(snake_case req body) |
| `app/src/transport/http.ts:115` | command→route 前缀映射表 |
| `app/src-tauri/src/commands/mod.rs:143` | `all_command_names` 注册表 |
| `app/e2e/fixtures.ts:218` + `app/e2e/README.md:84` | e2e route-mock 清单(catch-all miss = 500 fail-loud) |
| `app/src/stores/audit.ts` | 全量行 + 客户端 `sortEvents`(ts DESC, id DESC)+ 客户端过滤 + 三个 count getter |
| `app/src/components/audit/AuditLogModal.vue` | chip「X / Y 项」、critical label「(N)」、列表 v-for |
| `app/src/stores/traceStore.ts:302` | 全量拉取按 turnSeq 分组(**不动**) |
| 索引 | `idx_session_audit_events_session_ts(session_id, ts)` — keyset 的 ts 段可走索引,id tie-break 在过滤后子集上排序(页级量,可接受) |

## 1. 定案:新增 keyset 分页命令(additive)

**决策 D1 — keyset `(ts, id)` 游标,不用 OFFSET**:
审计表 append-only 且按新→旧消费;弹窗开着时 agent 可能持续写新行。OFFSET
分页在「取页后前移插入新行」时整页位移(重复+跳行);keyset 游标锚定
`(ts, id)`,旧页稳定。ts 是 `datetime('now')` 秒级精度,同秒多行是常态
(单轮多 tool),游标必须带 id 段:`(ts < :t OR (ts = :t AND id < :i))`。

**决策 D2 — 过滤/计数下推 SQL,不是前端过滤已载页**:
现行 chip 语义是「对全量行计数」。若过滤只作用于已载页,计数对未载行失真、
「加载更多」续拉的行也不保证满足过滤。下推后:列表页、matched、续拉口径
三者天然一致。kind = 等值列;critical = `json_valid(payload_json) AND
json_extract(payload_json, '$.critical') = 1`(SQLite json_extract 对 JSON
true 返回 1;`json_valid` 前置守卫,R7 的 NULL/畸形 JSON → NULL → 非 critical,
不会 error)。

**决策 D3 — 新增命令,不改旧命令**:返回形状从数组变 page 对象是 breaking
wire change,且 traceStore 仍需全量。新增
`list_session_audit_events_page`,旧命令零改动(R6);未来若 trace 面要治理
另行立项。

**决策 D4 — 前端「加载更多」按钮,不做无限滚动/虚拟滚动**(C2/C3):弹窗
滚动容器底部一个按钮,可见性 = `hasMore`(客户端派生:`events.length < matched`),
无 scroll 监听/sentinel 复杂度,可测性好。

## 2. 后端形状

### db 层(`db/permissions.rs` 新增,旧函数不动)

```rust
pub struct AuditEventPageQuery {
    pub limit: Option<i64>,          // default 100, cap 500
    pub before_ts: Option<String>,   // cursor 段 1(ts 文本,SQLite datetime 串)
    pub before_id: Option<i64>,      // cursor 段 2;before_ts Some 时必填
    pub kind: Option<String>,        // 类别过滤
    pub critical_only: bool,         // 仅 critical
}

pub struct AuditEventPageRow {      // #[serde(rename_all = "camelCase")]
    pub events: Vec<AuditEventRow>, // ORDER BY ts DESC, id DESC
    pub matched: i64,               // 当前过滤命中总数
    pub total_all: i64,             // 不过滤总数(= 现 totalCount)
    pub total_critical: i64,        // critical 总数,不受 kind 影响(= 现 criticalCount)
}

pub async fn list_audit_events_page(pool, session_id, q) -> Result<AuditEventPageRow, sqlx::Error>
```

页查询(动态拼段,全部 bind):

```sql
SELECT id, session_id, ts, kind, payload_json, turn_seq
FROM session_audit_events
WHERE session_id = ?
  [AND kind = ?]
  [AND (json_valid(payload_json) AND json_extract(payload_json,'$.critical') = 1)]
  [AND (ts < ? OR (ts = ? AND id < ?))]      -- before_ts Some 时
ORDER BY ts DESC, id DESC
LIMIT ?
```

计数两条(同过滤谓词复用):
- `total_all + total_critical`:一条 `SELECT COUNT(*) AS total_all,
  COUNT(*) FILTER (WHERE json_valid(payload_json) AND json_extract(...) = 1)
  AS total_critical WHERE session_id = ?`(SQLite 3.30+ 支持 FILTER)。
- `matched`:COUNT + 当前 kind/critical 谓词。

### command 层(`commands/permissions.rs`)

`list_session_audit_events_page` + `_inner`(双 transport 共用,house style)。
Tauri 参数 camelCase:`sessionId, limit?, beforeTs?, beforeId?, kind?, criticalOnly?`。

### daemon 路由(`daemon/routes/permissions.rs`)

POST `/list_session_audit_events_page`,req struct snake_case(镜像同文件
`list_session_audit_events` 的 `req.session_id` 风格),注册进 router;
`http.ts` 命令表加 `list_session_audit_events_page: "permissions"`;
`commands/mod.rs::all_command_names` 加行;`app/e2e/fixtures.ts` 加 mock
(`{ events: [], matched: 0, totalAll: 0, totalCritical: 0 }`)+ README 表行。

## 3. 前端形状

### `stores/audit.ts` 重构(保留 store 名与对外 actions 名)

- 状态:`events`(累计页,SQL 已序,删 `sortEvents`)、`matched/totalAll/totalCritical`、
  `loading/loadingMore/error/lastSessionId`、`kindFilter/onlyCritical`(语义不变)。
- `loadForSession(sid)`:reset 后拉第一页(kindFilter/onlyCritical 随行下推)。
- `loadMore()`:游标 = 已载最后一行的 `(ts, id)`,append;`hasMore = events.length < matched`。
- `setKindFilter / toggleCritical`:更新过滤 + 从第一页重拉(R2;异步,失败保旧值
  沿用现行 error 策略)。
- `refresh()`:重拉第一页(现行手动刷新语义,重锚到最新)。
- getters:`filteredCount → matched`、`totalCount → totalAll`、`criticalCount → totalCritical`
  (名字不变,Modal 计数 chip 零改动)。
- `isCritical` 保留(行内徽标渲染仍需要;count 职责移交服务端)。

### `AuditLogModal.vue`

- 列表尾部加「加载更多」按钮(`v-if="store.hasMore"`)+ 加载中态;点击
  `store.loadMore()`。样式走 `.btn` 家族 + 现有 token(镜像刷新按钮)。
- 过滤控件/watch 逻辑不动(store setter 内部语义变为服务端过滤)。

## 4. 兼容与回滚

- wire additive:旧命令/旧形状零改动;新命令独立注册。旧 daemon + 新前端组合
  会走 catch-all 404 → store error 路径,弹窗可见错误横幅,不 crash(与现行
  IPC 失败策略一致)。
- 回滚 = revert 前端消费点(改回旧命令)或整体 revert;无数据/迁移遗留。

## 5. 测试映射(对 AC)

| AC | 测试 |
|---|---|
| AC1 | `permissions_tests.rs`:tie 排序;cursor 续拉「取页后 insert 新行」不重不漏;limit cap |
| AC2 | kind 过滤;critical 四态(true/false/NULL payload/畸形 JSON,畸形不 error) |
| AC3 | 三 count 精确 + 过滤组合 matched |
| AC4 | 注册五处齐;`cargo test --test e2e` 绿;fixtures mock 与 wire 形状一致 |
| AC5 | `stores/audit.test.ts`(新增)+ `AuditLogModal.test.ts`(新增,镜像 PermissionGrantsModal.test.ts 先例) |
| AC6 | 手动冒烟(dev DB 长会话 + `sqlite3 -readonly` 交叉核对) |
| AC7 | 全量后端/前端套件 + clippy/fmt/vue-tsc |
| AC8 | DEBT.md 删条目(commit 内) |
