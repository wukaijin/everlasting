1. 顶栏最大化功能有问题，最大只能1290*1080，大于这个尺寸的屏幕无法铺满
2. app启动后，shell 日志有3条警告
  ```
libEGL warning: MESA-LOADER: failed to open vgem: /usr/lib/dri/vgem_dri.so: cannot open shared object file: No such file or directory (search paths /usr/lib/x86_64-linux-gnu/dri:\$${ORIGIN}/dri:/usr/lib/dri, suffix _dri)

3:39:11 PM [vite] (client) warning: <button> cannot be child of <button>, according to HTML specifications. This can cause hydration errors or potentially disrupt future functionality.
84 |            title="正在生成"
85 |          >●</span>
86 |          <button
   |          ^^^^^^^
87 |            class="tab__close"
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
88 |            :title="'关闭 Tab(数据保留)'"
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
89 |            :aria-label="`关闭 ${p.name}`"
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
90 |            @click="(e) => onHide(p.id, e)"
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
91 |          >
   |  ^^^^^^^^^
92 |            <Icon name="x" :size="12" />
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
93 |          </button>
   |  ^^^^^^^^^^^^^^^^^
  Plugin: vite:vue
  File: /usr/local/code/github/everlasting/app/src/components/ProjectTabs.vue
[@vue/compiler-sfc] `withDefaults` is a compiler macro and no longer needs to be imported.
  ```
3. 如图所示， @THINGKING-BLOCK.png， thinking block 打开后显示的css有问题。
