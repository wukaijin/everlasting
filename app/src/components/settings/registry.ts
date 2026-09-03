// registry — SettingsModal 侧边导航的分类元数据(2026-08-29
// settings-shell 重构)。
//
// 壳从 8 个横向 tab 改为「搜索 + 全局/项目 scope + 左侧分组导航 +
// 右侧内容区」;每个分类对应一个自包含的内容组件(原 tab 组件原样
// 复用,本文件只承载导航与搜索所需的描述性元数据)。组件映射
// (id → Vue component)留在 SettingsModal.vue —— registry 保持
// 纯数据 + 纯函数,搜索过滤可直接单测。

/** 设置的两个 scope。后端设置存全是 daemon 级全局 KV(`app_config`),
 *  「项目」scope 只收编真实项目级的数据:项目指令文件
 *  (`<project>/CLAUDE.md` + `AGENTS.md`)与项目子代理定义
 *  (`<project>/.everlasting/agents/*.md`,frontmatter `model:` 真项目级)。 */
export type SettingsScope = "global" | "project";

/** 导航分组标签。`null` = 不入组的独立首项(通用)。
 *  「存储」(F3 磁盘治理,2026-09-03):磁盘占用 / 回收开关 / 手动清理。 */
export type SettingsGroup = "模型" | "智能体" | "集成" | "存储" | "远程";

export interface SettingsCategory {
  /** 稳定 id:内容组件映射 + localStorage 记忆上次停留位置共用。 */
  id: string;
  scope: SettingsScope;
  /** 所属分组;null = 顶部独立项。 */
  group: SettingsGroup | null;
  /** 侧边导航与内容区标题(沿用原 tab 文案,保持用户肌肉记忆)。 */
  title: string;
  /** 内容区标题下的一行说明。 */
  description: string;
  /** 搜索关键词(中英文别名 + 内部小节名)。匹配对 title /
   *  description / keywords 做 size-insensitive 的 includes。 */
  keywords: string[];
}

/** 分组展示顺序(全局 scope)。未列出的组排在其后。存储排在集成之后、
 *  远程之前:同属本地应用关注点,且「远程」保持收尾。 */
export const SETTINGS_GROUP_ORDER: ReadonlyArray<SettingsGroup> = [
  "模型",
  "智能体",
  "集成",
  "存储",
  "远程",
];

export const SETTINGS_CATEGORIES: ReadonlyArray<SettingsCategory> = [
  {
    id: "general",
    scope: "global",
    group: null,
    title: "通用",
    description: "通知与全局调度的总开关。",
    keywords: ["general", "通知", "toast", "开关", "调度", "定时任务总开关", "notify"],
  },
  {
    id: "providers",
    scope: "global",
    group: "模型",
    title: "Providers",
    description: "LLM 服务商接入:协议、Base URL 与 API Key。",
    keywords: ["provider", "服务商", "api key", "base url", "anthropic", "openai", "密钥"],
  },
  {
    id: "models",
    scope: "global",
    group: "模型",
    title: "Models",
    description: "按服务商管理模型,支持连通性测试。",
    keywords: ["model", "模型", "测试", "连通", "connectivity"],
  },
  {
    id: "default",
    scope: "global",
    group: "模型",
    title: "Default",
    description: "挑选新会话默认使用的模型。",
    keywords: ["default", "默认模型", "缺省"],
  },
  {
    id: "memory",
    scope: "global",
    group: "智能体",
    title: "Memory",
    description: "用户级指令文件(User CLAUDE.md / AGENTS.md)预览。",
    keywords: ["memory", "记忆", "指令文件", "claude.md", "agents.md", "用户指令"],
  },
  {
    id: "subagents",
    scope: "global",
    group: "智能体",
    title: "Subagents",
    description: "为每个子代理(内置 / 用户 / 项目)覆盖默认模型。",
    keywords: ["subagent", "子代理", "模型覆盖", "override", "worker"],
  },
  {
    id: "search",
    scope: "global",
    group: "集成",
    title: "Search",
    description: "web_search 工具的提供方(DDG / Tavily)与 API Key。",
    keywords: ["search", "搜索", "联网", "tavily", "ddg", "duckduckgo", "web"],
  },
  {
    id: "scheduled",
    scope: "global",
    group: "集成",
    title: "定时任务",
    description: "跨项目管理定时任务:启停、编辑与运行状态。",
    keywords: ["scheduled", "定时", "任务", "cron", "调度", "触发"],
  },
  {
    id: "disk",
    scope: "global",
    group: "存储",
    title: "磁盘",
    description: "磁盘占用概览、自动回收开关与手动清理。",
    keywords: [
      "disk",
      "磁盘",
      "存储",
      "空间",
      "占用",
      "清理",
      "回收",
      "cleanup",
      "usage",
    ],
  },
  {
    id: "remote",
    scope: "global",
    group: "远程",
    title: "Remote",
    description: "远程隧道配置、连接状态、设备配对与节点信息。",
    keywords: ["remote", "远程", "手机", "隧道", "tunnel", "配对", "pairing", "节点", "node"],
  },
  {
    id: "project-memory",
    scope: "project",
    group: null,
    title: "项目指令文件",
    description: "当前项目的 CLAUDE.md / AGENTS.md 指令层预览。",
    keywords: ["memory", "记忆", "指令文件", "claude.md", "agents.md", "项目指令", "project"],
  },
  {
    id: "project-sandbox",
    scope: "project",
    group: null,
    title: "项目沙盒",
    description: "当前项目的沙盒策略档:放行 / 读写(默认)/ 只读。",
    keywords: ["sandbox", "沙盒", "隔离", "策略", "只读", "读写", "放行", "landlock", "project"],
  },
  {
    id: "project-subagents",
    scope: "project",
    group: null,
    title: "项目子代理",
    description: "本项目 .everlasting/agents/ 下定义的子代理及其模型。",
    keywords: ["subagent", "子代理", "agents", "模型", "project", "项目"],
  },
];

/** Lowercase + 全角空格归一;中文无大小写,统一 toLowerCase 即可。 */
function normalize(s: string): string {
  return s.toLowerCase().replace(/\u3000/g, " ").trim();
}

/** 按 scope 取全部分类(保持声明顺序)。 */
export function categoriesForScope(scope: SettingsScope): ReadonlyArray<SettingsCategory> {
  return SETTINGS_CATEGORIES.filter((c) => c.scope === scope);
}

/** 搜索过滤:空查询返回该 scope 全部分类;非空时对 title /
 *  description / keywords 做归一化 includes 匹配。纯函数。 */
export function filterCategories(
  query: string,
  scope: SettingsScope,
): ReadonlyArray<SettingsCategory> {
  const pool = categoriesForScope(scope);
  const q = normalize(query);
  if (!q) return pool;
  return pool.filter(
    (c) =>
      normalize(c.title).includes(q) ||
      normalize(c.description).includes(q) ||
      c.keywords.some((k) => normalize(k).includes(q)),
  );
}

/** 把扁平分类列表组装成侧边导航的分组视图:独立项(group=null)在
 *  最前,其余按 SETTINGS_GROUP_ORDER 排序;组内保持声明顺序。 */
export function groupCategories(
  categories: ReadonlyArray<SettingsCategory>,
): ReadonlyArray<{ label: string | null; items: ReadonlyArray<SettingsCategory> }> {
  const standalone = categories.filter((c) => c.group === null);
  const groups: Array<{ label: string | null; items: SettingsCategory[] }> = [];
  if (standalone.length > 0) {
    groups.push({ label: null, items: [...standalone] });
  }
  const groupedLabels = [
    ...SETTINGS_GROUP_ORDER,
    ...new Set(
      categories
        .map((c) => c.group)
        .filter((g): g is SettingsGroup => g !== null && !SETTINGS_GROUP_ORDER.includes(g)),
    ),
  ];
  for (const label of groupedLabels) {
    const items = categories.filter((c) => c.group === label);
    if (items.length > 0) {
      groups.push({ label, items });
    }
  }
  return groups;
}

/** 按 id 查分类(找不到返回 undefined —— localStorage 里存的 id 可能
 *  因分类更名/删除而失效,调用方需回退默认值)。 */
export function findCategory(id: string): SettingsCategory | undefined {
  return SETTINGS_CATEGORIES.find((c) => c.id === id);
}

/** 新分类加入时的默认落点:全局 scope 首项。 */
export const DEFAULT_CATEGORY_ID = "general";
