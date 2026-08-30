# Implement — RULE-PERM-001 审计事件查询分页

> 单分支顺序执行;PR1/PR2 后端、PR3 前端,每步末尾带验证命令。
> WSL 跑后端测试记得 `PKG_CONFIG_PATH`(见 docs/HACKING-wsl.md 坑 1)。

## PR1 — db 层:keyset 分页 + 过滤 + 计数

- [x] `db/permissions.rs` 新增 `AuditEventPageQuery` / `AuditEventPageRow`
  (camelCase wire)与 `list_audit_events_page`(动态拼段全 bind;谓词与
  SQL 形状按 design §2;`json_valid` 守卫;`ORDER BY ts DESC, id DESC`;
  limit default 100 cap 500)。旧 `list_audit_events` 零改动。
- [x] `db/permissions_tests.rs` 新增测试(映射 AC1/AC2/AC3):
  - `audit_page_orders_ts_desc_id_desc_tie_break`(同秒多行)
  - `audit_page_keyset_stable_when_new_row_appended_mid_pagination`(取页 1 →
    insert 更新行 → 游标取页 2 → 全序无重无漏;OFFSET 对照注释)
  - `audit_page_respects_limit_and_caps`
  - `audit_page_kind_filter_matches_subset`
  - `audit_page_critical_filter_four_states`(true/false/NULL payload/畸形 JSON
    不 error)
  - `audit_page_counts_exact_with_and_without_filters`
  - `audit_page_empty_session_returns_zeroed_page`
- 验证:`cargo test -p everlasting --lib permissions_tests`(带 PKG_CONFIG_PATH)

## PR2 — 双 transport 接线(additive 五处)

- [x] `commands/permissions.rs`:`list_session_audit_events_page_inner` + Tauri
  command(参数 camelCase;镜像同文件既有 `_inner` 模式)
- [x] `daemon/routes/permissions.rs`:req struct(snake_case)+ handler + router 注册
- [x] `commands/mod.rs::all_command_names` 加行
- [x] `app/src/transport/http.ts` 命令表加 `list_session_audit_events_page: "permissions"`
- [x] `app/e2e/fixtures.ts` 加 mock(形状 = AuditEventPageRow camelCase)+
  `app/e2e/README.md` 路由表加行
- [x] daemon route 层测试(镜像同文件既有 route 测试先例,如有)
- 验证:`cargo test -p everlasting --test e2e` + `cargo test -p everlasting --lib` 全量

## PR3 — 前端:store 重构 + Modal「加载更多」

- [x] `stores/audit.ts` 按 design §3 重构(删 sortEvents 客户端重排;getters
  名字不变接服务端 count;loadMore 游标;filter setter 重拉)
- [x] `AuditLogModal.vue` 列表尾部「加载更多」按钮(v-if hasMore,`.btn` 家族 +
  design token,镜像刷新按钮行)
- [x] 新增 `stores/audit.test.ts`(fake transport,镜像既有 store 测试先例):
  首屏 reset+加载、loadMore 游标 append、过滤变更重拉第一页、hasMore 终止、
  count 映射、错误保旧值
- [x] 新增 `components/audit/AuditLogModal.test.ts`(镜像 PermissionGrantsModal
  先例):hasMore ↔ 按钮可见性、点击触发 loadMore、chip 文案
- 验证:`cd app && pnpm test` + `pnpm vue-tsc --noEmit`(或 CI 等价)+ build

## 收尾

- [x] AC6 手动冒烟:daemon 起 dev 环境,开长会话审计弹窗,加载更多到末尾,
  计数与 `sqlite3 -readonly` 查询交叉核对;过滤切换重拉。
- [x] AC7 全量:后端 lib + fmt + clippy `-D warnings`;前端 vitest + vue-tsc。
- [x] Phase 3.3 spec 更新评估(候选:`database-guidelines.md` keyset 分页模式 /
  `daemon-server.md` 新路由登记惯例)。
- [x] AC8:`.trellis/reviews/DEBT.md` 删 §RULE-PERM-001 条目 + 优先级分布表归零。

## 回滚点

- 每个 PR 独立可 revert;PR3 revert 后弹窗回到旧全量命令(旧命令未动,零残留)。
