# 技能:web-debug-source-locate
触发:网页调试窗框选元素后,需要把选中的 DOM 反向定位到本地源码再修改时

把"渲染出来的 DOM"反查到"本地源码哪一处"是网页调试的核心难点。按下面顺序,先用最准的、再退化:

1. **先看现成坐标**:选中元素及其祖先有没有 `data-source` / `data-v-inspector` / `__source` 之类
   带文件:行号的属性。有就直接用——最准,不用猜。
2. **判工程类型**(读 package.json / 配置文件,用 file_read / code_search):
   - Vite + React → 用 shell 装 `react-dev-inspector` 或 `vite-plugin-dev-inspector`,改 vite.config
     注入插件,重启 dev server → 之后渲染元素自带源码坐标,回到第 1 步。
   - Vite + Vue → `vite-plugin-vue-inspector`,同路。
   - EJS / Express / 无插件生态 → 走第 3 步。
3. **code_search 三段法**(静态能中,动态模板插值要搜锚点不搜渲染值):
   - 先按选中元素的**可见文字**精确搜;
   - 不中再按 **class / id / 结构特征**搜;
   - 还不中用**父链锚点**组合缩小范围。
   - 动态内容(`<%= %>` / `{{ }}` 等模板插值)别搜渲染后的值(搜不到),搜它周边的**静态文字/标签**,
     模板目录(views/ 之类)优先。
4. **改完源码** → 调 reload_debug_webview 刷新(EJS/Express 无 HMR,必须刷)→ 再框选复核改对没有
   (自我负责:别只信"我改了",回去看渲染结果)。

要点:这是知识不是机制——你用的是通用工具(shell 装插件 / file_edit 改源 / code_search 定位),
不存在"某框架专用的反查工具"。新框架来了,就在判断里多加一条分支。
