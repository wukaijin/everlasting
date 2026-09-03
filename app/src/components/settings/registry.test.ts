// Tests for `registry.ts` — SettingsModal 侧边导航的分类元数据与
// 搜索过滤(settings-shell 重构,2026-08-29)。
//
// 契约:
//   1. filterCategories 空查询返回该 scope 全部分类(声明顺序)。
//   2. 中英文关键词命中:对 title / description / keywords 归一化
//      includes 匹配(大小写不敏感)。
//   3. scope 隔离:全局查询不串到项目分类,反之亦然。
//   4. groupCategories:独立项(group=null)在最前,组按声明顺序,
//      空组不出现。
//   5. findCategory 未知 id 返回 undefined(localStorage 失效回退
//      依赖此行为)。

import { describe, it, expect } from "vitest";
import {
  SETTINGS_CATEGORIES,
  DEFAULT_CATEGORY_ID,
  categoriesForScope,
  filterCategories,
  groupCategories,
  findCategory,
} from "./registry";

describe("settings registry", () => {
  it("空查询返回该 scope 全部分类", () => {
    const globalCats = filterCategories("", "global");
    expect(globalCats).toHaveLength(categoriesForScope("global").length);
    expect(globalCats[0]?.id).toBe(DEFAULT_CATEGORY_ID);

    const projectCats = filterCategories("   ", "project");
    expect(projectCats.map((c) => c.id)).toEqual(["project-memory", "project-sandbox", "project-subagents"]);
  });

  it("中英文关键词命中 title / description / keywords", () => {
    // title 命中(分类名)
    expect(filterCategories("providers", "global").map((c) => c.id)).toContain("providers");
    // 中文关键词命中 keywords(「配对」是 Remote 的内部小节名)
    expect(filterCategories("配对", "global").map((c) => c.id)).toContain("remote");
    // 英文别名大小写不敏感
    expect(filterCategories("Tunnel", "global").map((c) => c.id)).toContain("remote");
    // description 命中(「总开关」出现在通用分类描述里)
    expect(filterCategories("总开关", "global").map((c) => c.id)).toContain("general");
    // 中文 title
    expect(filterCategories("定时", "global").map((c) => c.id)).toContain("scheduled");
    // F3(2026-09-03):磁盘分类的中英文关键词命中。
    expect(filterCategories("磁盘", "global").map((c) => c.id)).toContain("disk");
    expect(filterCategories("cleanup", "global").map((c) => c.id)).toContain("disk");
  });

  it("无匹配返回空数组", () => {
    expect(filterCategories("不存在的设置xyzzy", "global")).toEqual([]);
  });

  it("scope 隔离:互不串扰", () => {
    // 「Memory」在两个 scope 都有,但过滤只返回当前 scope。
    for (const cat of filterCategories("memory", "global")) {
      expect(cat.scope).toBe("global");
    }
    for (const cat of filterCategories("memory", "project")) {
      expect(cat.scope).toBe("project");
    }
    // 全部注册分类的 scope 恰好覆盖两类。
    expect(new Set(SETTINGS_CATEGORIES.map((c) => c.scope))).toEqual(
      new Set(["global", "project"]),
    );
  });

  it("groupCategories:独立项在最前,空组不出现,组内保持声明顺序", () => {
    const groups = groupCategories(categoriesForScope("global"));
    // 首组 = 独立项(通用),无组标签。
    expect(groups[0]).toEqual({ label: null, items: [expect.objectContaining({ id: "general" })] });
    const labels = groups.slice(1).map((g) => g.label);
    // F3(2026-09-03):新增「存储」组(集成之后、远程之前)。
    expect(labels).toEqual(["模型", "智能体", "集成", "存储", "远程"]);
    // 模型组内顺序:Providers → Models → Default。
    const modelGroup = groups.find((g) => g.label === "模型");
    expect(modelGroup?.items.map((c) => c.id)).toEqual(["providers", "models", "default"]);
    // 存储组内:磁盘。
    const diskGroup = groups.find((g) => g.label === "存储");
    expect(diskGroup?.items.map((c) => c.id)).toEqual(["disk"]);
    // 项目 scope 无分组:两个独立项合并成一个无标签组。
    const projectGroups = groupCategories(categoriesForScope("project"));
    expect(projectGroups).toHaveLength(1);
    expect(projectGroups[0]?.label).toBeNull();
  });

  it("findCategory:已知 id 命中,未知 id 返回 undefined", () => {
    expect(findCategory("general")?.scope).toBe("global");
    expect(findCategory("project-memory")?.scope).toBe("project");
    expect(findCategory("no-such-category")).toBeUndefined();
  });
});
