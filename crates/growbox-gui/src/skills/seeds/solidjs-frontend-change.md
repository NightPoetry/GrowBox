# 技能:solidjs-frontend-change
触发:动 GrowBox 自身前端(crates/growbox-gui/frontend,改组件/状态/样式)时

前端是 **SolidJS,不是 React**——改前先认清,否则会写错范式。

1. **SolidJS 心智 ≠ React**:用 **signal**(`createSignal`)不是 hooks;**细粒度响应式**,不重渲整个组件树(改 signal 只更新依赖它的 DOM 节点);`createEffect` 跑副作用;`<Show>`/`<For>` 而非三元/map+key 的 React 套路。别套 useState/useEffect/虚拟 DOM 的思路。
2. **代码住哪(改前先 code_search 找对地方)**:全局状态信号在 `store.ts`;所有 Tauri 调用经 `tauri-api.ts` 唯一网关(invoke 都在这);toast/提示走 `notices.ts`(单一源 notices.i18n.json,双受众);界面文案走 `i18n.ts` + `locales/*.ts`(四语);设置各 Tab 在 `components/settings/`,逻辑信号在 `settings/state.ts`。
3. **★铁律:改前端不清 `~/Library/WebKit/`★**:那里是 localStorage,存着连接配置(API 地址/key/模型)+ 全部旋钮——清了用户配置全回退默认(本项目血泪)。重测换数据用别的办法。
4. **新文案补全四语 + 过完整性守卫**:加界面文字要在 4 个 locale 都补(zh-CN/en/ja/zh-TW),否则 i18n 守卫测试红;走 `t("key")` 不硬编码。
5. **像周围组件那样写**:命名、样式(内联 style 对象 / class)、结构跟着该组件的现有风格;特殊语义内容给醒目样式(见 [[ui-respects-attention]])。
6. **改完真验**:`npx tsc --noEmit` + `npm run build` 必过;UI 行为用全自动调试模式驱动核对(见 [[self-test-with-auto-debug]]),别只信"应该渲对了"。

要点:认准 SolidJS 范式 + 改对地方(store/tauri-api/notices/i18n 各司其职)+ 死守"不清 WebKit",是动这个前端不翻车的三件套。
