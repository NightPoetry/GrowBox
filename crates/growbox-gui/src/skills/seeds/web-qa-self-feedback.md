# 技能:web-qa-self-feedback
触发:要测自己做的网页功能对不对(按钮跳转/表单提交/各种交互),像真人测试员有计划地真操作再核对时

只看 CSS、只枚举回调,发现不了"按钮跳转坏、表单提交到错页、点了没反应"这类功能 bug(用户实测:CSS 没问题了,按钮跳转却坏了)。要像真人 QA 一样**真点、真填、真提交,再读回结果核对**。手 = web_debug_drive 工具(在调试窗里 click/fill/submit/scan/observe);眼 = 它返回的 url / title / 本页报错 / lastAction。

1. **先开调试窗**:看清项目里的 web 工程(读 package.json 找 dev 脚本)→ 用 shell **后台**起 dev server → 从输出抓本地 URL → 调 open_debug_url 把它拉进调试窗。
2. **列测试计划**:web_debug_drive{op:"scan"} 枚举当前页的交互点(按钮/链接/表单/输入,带文本与 name/id)。据此列一份"要验哪些功能"的清单——别漫无目的乱点,有计划地一个个过。
3. **逐个真操作 + 核对**:
   - 按钮/链接:web_debug_drive{op:"click", selector} → 看返回的 url 跳对没、title 对不对、errors 有没有报错、lastAction.matched 是不是真点到了。**matched=false = 选择器没匹配(根本没点到),不是功能 bug** → 改选择器重试,别误判成"功能坏了"。
   - 表单:web_debug_drive{op:"fill", selector, value} 逐个填字段 → web_debug_drive{op:"submit", selector} 提交 → 再 observe,看提交后到没到对的页、有没有冒出验证错误。
   - 只读核对当前页:web_debug_drive{op:"observe"}。
4. **selector 写法**:是 CSS 选择器,**别用带单引号的**(如 a[href='/x'] 会破坏注入、querySelector 抛错)→ 用 a[href*=archive]、button.submit、#email 这类无单引号写法。先用 scan 看清 id/name/文本,再据此选。
5. **发现 bug 就修再复测**:功能不对(跳错页/报错/提交失败)→ 用 code_search 把现象反查到本地源码、file_edit 改 → reload_debug_webview 刷新(EJS/Express 无 HMR 靠它看到改动)→ **重跑第 3 步那条用例确认修好**,别改完就当好了。见 [[web-debug-source-locate]] 做反向定位。
6. **★金融操作走授权闸★**:涉及真实金融交易(下单/支付/转账/扣款)的提交,**不要直接 submit**。先**栈调用内置工作流 `financial_action_gate`**(把要做的金融操作写进 `input`,用默认上下文别 isolated),读它返回的 `{authorized}`:`authorized=true` 才提交;`false` 就交回用户、**绝不替用户点**。它会查本项目授权历史,没有就让用户首次批准(批过本项目以后放行)。判不准"算不算金融"时就当金融走闸——宁可多问。
7. **出测试报告**:逐条交互点 pass/fail,fail 写清"点了什么 → 期望什么 → 实际什么(url/报错)"。别笼统说"测过了没问题"。

配合 [[verify-by-running]]:改完就自驱自验,别把"待真机目测"积压成虚假成功池。要点:你有手(web_debug_drive 真操作)、有眼(它回传 url/title/报错)——把功能点像真人 QA 一样过一遍,亲眼看它对不对,而不是只盯样式。
