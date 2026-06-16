# 技能:self-test-with-auto-debug
触发:改了 UI/交互,要自己验证它真能用(不靠人当肉调试器)时

GrowBox 测试包内建「全自动调试模式」——你能注入 UI 操作、内部截图、读事件埋点,自己驱动自己核对。

1. **出测试包并启动**:`scripts/build-test.sh`(带 debug-endpoints + VITE_GROWBOX_DEBUG)→ `open target/release/bundle/macos/GrowBox.app` → `curl http://127.0.0.1:19999/health` 探活。
2. **驱动 UI**:POST JS 表达式到 `127.0.0.1:19999/eval`(body 是**表达式**,多语句用 IIFE)。常用 `window.__GROWBOX__`:`clickElement(sel)` / `typeText(sel,文本)` 注入操作;`getState()`/`getDOM(sel)` 查状态;`waitFor(表达式,超时)` 等条件。
3. **读反馈三件套**:`screenshot()` 真内部截图(html2canvas→PNG dataUrl,非系统截图、零授权,存成 .png 亲眼看)/ `getEvents(n)` 事件埋点(chat-status/decision-request/notice/context-tokens…)/ `getToasts()` 在屏提示。
4. **连真 LLM 测**:连接配置在 localStorage(点 .btn-connect 即用);key 端点 192.168.x.x:8080/key。
5. **绕沙箱坑**:造物 iframe 是 null-origin,父页注入点不进、消息有 source 校验——测造物回调用造物内 setInterval 自 emit 触发,别从父页 postMessage 伪造。
6. **截图坑**:html2canvas 可能错层(把顺序堆叠画成重叠)→ 截图只作近似,**布局判断以 DOM `getBoundingClientRect` 为准**。
7. **HTTP /eval 超时 ~20s**:长轮询从 shell 侧切短块多次查,别在一次 eval 里 sleep 过 20s。

这套就是怎么"自己测自己"。配合 [[verify-by-running]]:改完就自驱自验,别把"待真机目测"积压成虚假成功池。

要点:你有手(注入操作)、有眼(内部截图)、有感知(事件埋点)——改完 UI 自己跑一遍证明它能用,不用等人。
