import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";

// WebSearchTab store(F4, task `08-25-web-search-tool` WP3)。
//
// Settings → 搜索 tab 是 `get/set_web_search_config` 两个 IPC 的纯 UI
// 包装(镜像 remoteConfig store 先例)。wire 形状 camelCase
//(Rust `WebSearchConfigPayload` 带 `#[serde(rename_all = "camelCase")]`)。
//
// key 三态语义(review P1-2)在 UI 侧的映射:
// - key 输入框**留空保存** → 不传 `tavilyApiKey` → 后端 `None`(不动);
// - 输入非空 key 保存 → 传明文 → 后端 `Some(非空)`(重加密落盘);
// - 点「清除已存 key」→ 传空串 → 后端 `Some("")`(删行,auto 回落 DDG)。
// GET 永不回明文——只有 masked(`tvly-****1234`)。

/** `get_web_search_config` payload(wire: camelCase)。 */
export interface WebSearchConfig {
  /** "auto" | "tavily" | "ddg"。 */
  provider: string;
  /** 密文行存在(≠ key 可解密:machine-id 变了仍为 true,只是不可预览)。 */
  tavilyKeySet: boolean;
  /** masked key,密文不可解密时为 null。 */
  tavilyKeyMasked: string | null;
}

export const useWebSearchStore = defineStore("webSearch", () => {
  const config = ref<WebSearchConfig | null>(null);
  const loaded = ref(false);

  /** Load the persisted web_search config. */
  async function load() {
    config.value = await transport.invoke<WebSearchConfig>(
      "get_web_search_config",
    );
    loaded.value = true;
  }

  /** 保存 provider;`tavilyApiKey` 仅在用户实际输入时传(undefined =
   *  后端 None = 不动已存 key)。args camelCase,httpTransport 顶层扳
   *  snake 后 daemon 结构体按 snake 反序列化。 */
  async function save(provider: string, tavilyApiKey?: string) {
    await transport.invoke("set_web_search_config", {
      provider,
      ...(tavilyApiKey !== undefined ? { tavilyApiKey } : {}),
    });
    await load();
  }

  /** 显式清除已存 key(Some("") 删行)。provider 原样带回——清除动作
   *  只动 key,不顺带改选路。 */
  async function clearKey() {
    await transport.invoke("set_web_search_config", {
      provider: config.value?.provider ?? "auto",
      tavilyApiKey: "",
    });
    await load();
  }

  return { config, loaded, load, save, clearKey };
});
