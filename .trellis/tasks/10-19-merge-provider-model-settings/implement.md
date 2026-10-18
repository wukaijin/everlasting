# Implement — 合并供应商/模型配置 + 模型探测 + 在线能力匹配

## 顺序

1. **后端业务 + 纯函数**(`commands/model_metadata.rs`)
   - wire 类型(`ModelSpec` / `ProbedModel`)
   - `ModelsCatalog`(索引、`best`、`matches`、`normalize_model_name`、`score_entry`、`provider_priority`、`guess_provider_key`)
   - 目录加载(缓存 / 网络 / 降级)
   - 两个 `_inner`
   - 12 个单测(含 `#[ignore]` 的 live 目录冒烟)
2. **后端接线**
   - `commands/mod.rs`:`pub mod model_metadata` + `all_command_names` 两条
   - `daemon/routes/providers.rs`:两个请求结构体 + handler + 两条 `.route`
   - `daemon/routes/mod.rs`:域注释计数
   - `transport/http.ts`:`CMD_TO_DOMAIN` 两条
   - `tests/e2e.rs`:路由 smoke 清单 + `e1f_model_probe` 三用例
3. **前端工具层**(`utils/modelMetadata.ts` + 测试)
4. **前端组件**
   - `ModelForm.vue`:三 prop + `auto-match` emit + 自动匹配行 + 固定 provider 标签
   - `ProviderModelsPanel.vue`:模型 CRUD + 测试 + 探测 + 导入 + 自动匹配编排
   - `ProvidersTab.vue`:卡片头部 / chevron / 模型数徽标 / 内嵌面板 + 样式
   - `registry.ts`:删 models + 别名表
   - `SettingsModal.vue`:摘除挂载
   - `ChatPanel.vue`:CTA 落点
   - 删除 `ModelsTab.vue`
5. **测试同步**:registry / SettingsModal / ChatPanel / ProvidersTab 既有用例按新形态改写 + 新用例
6. **验证**:`cargo fmt --check` → `cargo test` → `vue-tsc` → `pnpm test`
7. **live 冒烟**:临时 daemon + `lookup_model_metadata` 真实调用(顺带发现并修掉两个真实数据问题:`@` 日期拼法、聚合网关变体去重)

## 验证命令

```bash
cd app && pnpm vue-tsc --noEmit
cd app && pnpm test
cd app && pnpm vitest run src/utils/modelMetadata.test.ts src/components/settings/ProviderModelsPanel.test.ts src/components/settings/ProvidersTab.test.ts

cargo fmt -p everlasting -- --check
cargo test -p everlasting --lib                      # 2605+ (含新增 11)
cargo test -p everlasting --lib model_metadata::      # 窄跑
cargo test -p everlasting --test e2e e1f_model_probe::
cargo test -p everlasting --test e2e all_api_routes_are_mounted
# 可选(网络):
cargo test -p everlasting --lib model_metadata::tests::live -- --ignored
```

## 回滚点

- 后端:`commands/model_metadata.rs` + 五处接线可整块回退(不影响既有命令)。
- 前端:恢复 `ModelsTab.vue` + `SettingsModal` 的 `models` 映射 + registry 的 `models` 条目即回到双页形态;`ProviderModelsPanel` 删除。别名表可保留(无害)。

## 注意事项

- `models.dev` 的 JSON 对象遍历顺序**不可依赖** —— 去重与排序必须显式按 `(score, priority, name)` 裁决。
- 批量导入逐条 `add_model`,每条成功都会 `modelsStore.load()` 整表刷新:300 条上限是防呆上限,实际使用建议勾选 < 30 条。
- jsdom 无 `EventSource`、无真实网络:前端测试必须 mock `transport`;后端 e2e 用 `httpmock`,不触 models.dev。
