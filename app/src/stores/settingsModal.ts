import { defineStore } from "pinia";
import { ref } from "vue";

/** settingsModal — Settings 弹窗的全局打开通道(N1 首次引导,
 *  2026-09-15)。
 *
 *  此前「设置」入口只有 Sidebar footer 的本地 ref,聊天区空状态
 *  引导卡(R1.1)这类跨层入口够不着;provide/inject 跨
 *  ChatWindow→Sidebar 层级脆、事件总线无先例,pinia store 最顺
 *  (design §4.1)。
 *
 *  initialCategory 是一次性落点:openSettings(category) 记下目标
 *  分类,SettingsModal 打开时消费(直落该分类、本次跳过
 *  localStorage「上次停留」恢复)并经 consumeInitialCategory 清空
 *  —— 一次性语义,不污染用户上次停留记忆;普通入口(gear 按钮)
 *  行为不变。分类 id 见 components/settings/registry.ts
 *  (如 "providers" / "models")。 */
export const useSettingsModalStore = defineStore("settingsModal", () => {
  const open = ref(false);
  const initialCategory = ref<string | null>(null);

  /** 打开设置弹窗;category 非空时本次打开直落该分类(一次性)。 */
  function openSettings(category?: string) {
    initialCategory.value = category ?? null;
    open.value = true;
  }

  /** 消费一次性落点(SettingsModal 打开 watcher 调用):返回当前值
   *  并清空,保证只对一次打开生效。 */
  function consumeInitialCategory(): string | null {
    const c = initialCategory.value;
    initialCategory.value = null;
    return c;
  }

  return { open, initialCategory, openSettings, consumeInitialCategory };
});
