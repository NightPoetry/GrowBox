// 造物画布 —— AI 现造 UI 的渲染面(被造物展示半,Phase 1)。
//
// 实现 设计/05 推论8 + 计划/被造物-自由展示与交互.md Phase 1。
// 内容跑在沙箱 iframe:sandbox="allow-scripts" 但**不给 allow-same-origin** → null origin,
// 脚本能跑但够不到父页/本机存储/网络,纯展示安全。交互回调(流2)在 Phase 2 接。
// 导出 signal 供 ui-actions.ts 的 PANELS 注册表读(可被 ui_control 开/关/切)。

import { createSignal, Show, onMount, onCleanup, type Component } from "solid-js";
import { api } from "../tauri-api";
import { artifactReceiveRound, doSend } from "../chat";

// 展示态:modal 是否对用户可见(= AI"做好了才展示")。
export const [artifactCanvasOpen, setArtifactCanvasOpen] = createSignal(false);
// 渲染态:iframe 是否已有内容(草稿期 iframe 常驻 DOM 渲染+可 selftest,但 modal 仍隐藏)。
export const [artifactRendered, setArtifactRendered] = createSignal(false);
export const [artifactHtml, setArtifactHtml] = createSignal<string>("");
// 是否显示顶部横栏(窗口框架)。默认 true(硬性可拖横栏);chrome=auto 时 LLM 经 render_artifact 声明
// 可设 false(如桌宠);Settings 全局 always/never/auto 续点。横栏在则可拖动。
export const [artifactShowChrome, setArtifactShowChrome] = createSignal(true);

export function openArtifactCanvas() {
  // 仅在已渲染时展示(没内容不弹空壳)。
  if (artifactRendered()) setArtifactCanvasOpen(true);
}
export function closeArtifactCanvas() {
  // 非破坏性隐藏(AI 经 ui_control 关 / 仅收起):造物仍在,可再 open。不触发硬机制。
  setArtifactCanvasOpen(false);
}

// ★用户点 × 真关造物(造物交互 v2 §4 硬机制)★:卸载 iframe(artifactRendered=false → Show 收起 →
// onCleanup 自关 message 监听,硬性非 LLM)+ 通知后端(取消在跑造物回合 + AI 感知端口不通 + 用户 toast)。
// 区别于 closeArtifactCanvas(AI/ui_control 的非破坏性隐藏,不取消回合、不告知 AI)。
export function userCloseArtifact() {
  const wasRendered = artifactRendered();
  setArtifactCanvasOpen(false);
  setArtifactRendered(false);
  setArtifactHtml("");
  if (wasRendered) void api.artifactClosed("main").catch(() => {});
}
export function toggleArtifactCanvas() {
  if (artifactRendered()) setArtifactCanvasOpen((p) => !p);
}

// ★做好了才展示(用户 2026-06-04)★:回合结束时把已渲染的造物展示给用户。
// 中间多次 render_artifact 只更新草稿(不弹),AI finish/回合结束才 present 最终态 —— 用户不被"一边改一边弹"打断。
export function presentArtifactIfReady(): void {
  if (artifactRendered()) setArtifactCanvasOpen(true);
}

// 注入每个造物的 gx 引导脚本:事件委托捕获 [data-gx-callback],postMessage 回父页(流2)。
// 沙箱 null origin 下脚本能跑、只能 postMessage 出来。Phase 4 会在此加自检(枚举/合成触发)。
const GX_BOOTSTRAP = `<script>(function(){
  var gxRegistered={};
  function gxVal(el){
    if(el.hasAttribute('data-gx-value')) return el.getAttribute('data-gx-value');
    if('value' in el && el.value!=null) return String(el.value);
    return '';
  }
  function gxSend(id,value,realtime){
    if(!id) return;
    parent.postMessage({__gx:true,type:'event',callbackId:String(id),value:(value==null?'':String(value)),realtime:!!realtime},'*');
  }
  function gxEmit(el,realtime){
    if(!el) return;
    var id=el.getAttribute('data-gx-callback');
    if(!id) return;
    gxSend(id,gxVal(el),realtime);
  }
  // ★造物→AI 上报 API(canvas/动态计算的交互用)★:整块 <canvas> 是一个元素、落子是按坐标算棋格,
  // 没法给每格挂 data-gx-callback —— 此时在你的 JS 里调 window.gx.emit('回调名', 值) 上报给 AI,
  // 免手写易错的内部 postMessage 协议(漏 __gx/type 即静默失败)。register(id) 只声明(供 selftest 枚举,不发事件)。
  window.gx={
    emit:function(id,value){ gxRegistered[String(id)]=true; gxSend(id,value,false); },
    emitRealtime:function(id,value){ gxRegistered[String(id)]=true; gxSend(id,value,true); },
    register:function(id){ if(id!=null&&id!=='') gxRegistered[String(id)]=true; }
  };
  document.addEventListener('click',function(e){
    var el=e.target&&e.target.closest?e.target.closest('[data-gx-callback]'):null;
    if(el) gxEmit(el,false);
  });
  document.addEventListener('change',function(e){
    var el=e.target&&e.target.closest?e.target.closest('[data-gx-callback]:not([data-gx-realtime])'):null;
    if(el) gxEmit(el,false);
  });
  // 实时输入(data-gx-realtime):防抖 + 指数退避 + 仅内容变化才 emit(防挂机一直读烧 token)。
  var gxRT={};
  document.addEventListener('input',function(e){
    var el=e.target&&e.target.closest?e.target.closest('[data-gx-callback][data-gx-realtime]'):null;
    if(!el) return;
    var id=el.getAttribute('data-gx-callback'); if(!id) return;
    var s=gxRT[id]||(gxRT[id]={last:null,delay:1500});
    if(s.t) clearTimeout(s.t);
    if(s.reset) clearTimeout(s.reset);
    s.t=setTimeout(function(){
      var v=gxVal(el);
      if(v===s.last) return;                  // 内容没变 → 不 emit(用户停手/挂机不烧 token)
      s.last=v;
      gxEmit(el,true);
      s.delay=Math.min(s.delay*2,20000);      // 指数退避:连续变化 → 下次采样间隔翻倍(封顶 20s)
    },s.delay);
    s.reset=setTimeout(function(){ s.delay=1500; },Math.min(s.delay*2,40000)); // 安静一段后复位灵敏度
  });
  // ── 网页调试运行时:套索/点选 → DOM 快照(计划/网页调试窗-可视化框选改源 Phase 1)──
  // 父页发 {__gxcmd:'inspect',enable} 进/出选择态;选择态下 hover 高亮、点选单个/拖框选多个,
  // 收元素快照(选择器路径/outerHTML 截断/bbox)postMessage 回父页 {type:'inspect-select'}。
  var gxIns={on:false,ov:null,cv:null,ctx:null,draw:false,pts:[],hi:null,hiPrev:''};
  var gxHud=null;
  function gxHudShow(t){if(!gxHud){gxHud=document.createElement('div');gxHud.style.cssText='position:fixed;left:8px;top:8px;z-index:2147483647;background:rgba(0,0,0,0.82);color:#3f6;font:11px monospace;padding:4px 8px;border-radius:6px;pointer-events:none;white-space:pre';(document.body||document.documentElement).appendChild(gxHud);}gxHud.textContent='[套索] '+t;}
  function gxHudHide(){if(gxHud){gxHud.remove();gxHud=null;}}
  function gxMarkSel(els){for(var i=0;i<els.length;i++){(function(el){var pv=el.style.outline;el.style.outline='2px solid #22c55e';setTimeout(function(){try{el.style.outline=pv;}catch(e){}},1400);})(els[i]);}}
  function gxRect(el){var r=el.getBoundingClientRect();return {x:Math.round(r.left),y:Math.round(r.top),w:Math.round(r.width),h:Math.round(r.height)};}
  function gxSelector(el){
    if(!el||el.nodeType!==1) return '';
    var parts=[],cur=el,depth=0;
    while(cur&&cur.nodeType===1&&cur!==document.body&&depth<6){
      var seg=cur.tagName.toLowerCase();
      if(cur.id){parts.unshift('#'+cur.id);break;}
      if(typeof cur.className==='string'){var c=cur.className.trim().split(' ').filter(function(s){return !!s;}).slice(0,2).join('.');if(c)seg+='.'+c;}
      var p=cur.parentNode;
      if(p&&p.children){var same=[];for(var i=0;i<p.children.length;i++){if(p.children[i].tagName===cur.tagName)same.push(p.children[i]);}if(same.length>1)seg+=':nth-of-type('+(same.indexOf(cur)+1)+')';}
      parts.unshift(seg);cur=p;depth++;
    }
    return parts.join(' > ');
  }
  function gxSnap(el){
    var oh=(el.outerHTML||'');if(oh.length>240)oh=oh.slice(0,240)+'...';
    var tx=(el.textContent||'').trim();if(tx.length>80)tx=tx.slice(0,80)+'...';
    return {selector:gxSelector(el),tag:el.tagName.toLowerCase(),id:el.id||'',classes:(typeof el.className==='string'?el.className:''),text:tx,outerHTML:oh,rect:gxRect(el)};
  }
  function gxClrHi(){if(gxIns.hi){try{gxIns.hi.style.outline=gxIns.hiPrev;}catch(e){}gxIns.hi=null;}}
  function gxHi(el){if(el===gxIns.hi)return;gxClrHi();if(el&&el.nodeType===1){gxIns.hi=el;gxIns.hiPrev=el.style.outline;el.style.outline='2px solid #0a84ff';}}
  function gxAt(x,y){gxIns.ov.style.pointerEvents='none';var el=document.elementFromPoint(x,y);gxIns.ov.style.pointerEvents='auto';return (el&&el!==gxIns.ov)?el:null;}
  // 点在多边形内(射线法)——不规则套索的命中判定。
  function gxInPoly(x,y,pts){
    var inside=false,n=pts.length,j=n-1;
    for(var i=0;i<n;i++){var xi=pts[i].x,yi=pts[i].y,xj=pts[j].x,yj=pts[j].y;
      if(((yi>y)!==(yj>y))&&(x<(xj-xi)*(y-yi)/(yj-yi)+xi))inside=!inside;j=i;}
    return inside;
  }
  // 套索多边形内的元素:质心落在多边形内即命中,先用包围盒粗筛;取叶子(去掉其他命中元素的祖先),限 12。
  function gxInLasso(pts){
    var minx=1e9,miny=1e9,maxx=-1e9,maxy=-1e9;
    for(var k=0;k<pts.length;k++){var p=pts[k];if(p.x<minx)minx=p.x;if(p.x>maxx)maxx=p.x;if(p.y<miny)miny=p.y;if(p.y>maxy)maxy=p.y;}
    var hit=[],all=document.body?document.body.querySelectorAll('*'):[];
    for(var i=0;i<all.length;i++){var b=all[i].getBoundingClientRect();
      if(b.width===0||b.height===0)continue;
      var cx=b.left+b.width/2,cy=b.top+b.height/2;
      if(cx<minx||cx>maxx||cy<miny||cy>maxy)continue;
      if(gxInPoly(cx,cy,pts))hit.push(all[i]);}
    var leaf=[];for(var a=0;a<hit.length;a++){var anc=false;for(var c2=0;c2<hit.length;c2++){if(a!==c2&&hit[a].contains(hit[c2])){anc=true;break;}}if(!anc)leaf.push(hit[a]);}
    return leaf.slice(0,12);
  }
  // 重绘套索手绘路径(虚线描边 + 半透明填充)。
  function gxRedraw(){
    var c=gxIns.ctx;if(!c)return;c.clearRect(0,0,gxIns.cv.width,gxIns.cv.height);
    var pts=gxIns.pts;if(pts.length<2)return;
    c.beginPath();c.moveTo(pts[0].x,pts[0].y);
    for(var i=1;i<pts.length;i++)c.lineTo(pts[i].x,pts[i].y);
    c.closePath();
    c.fillStyle='rgba(10,132,255,0.12)';c.fill();
    c.strokeStyle='#0a84ff';c.lineWidth=1.5;c.setLineDash([5,4]);c.stroke();
  }
  function gxStart(){
    if(gxIns.on)return;gxIns.on=true;
    var ov=document.createElement('div');ov.style.cssText='position:fixed;inset:0;z-index:2147483600;cursor:crosshair;background:transparent;user-select:none;-webkit-user-select:none;';document.body.appendChild(ov);gxIns.ov=ov;
    var cv=document.createElement('canvas');cv.width=window.innerWidth;cv.height=window.innerHeight;
    cv.style.cssText='position:fixed;left:0;top:0;z-index:2147483601;pointer-events:none;';document.body.appendChild(cv);gxIns.cv=cv;gxIns.ctx=cv.getContext('2d');
    // 套索期间掐死原生选区(那个"框选框"),否则它抢走拖拽、套索点收不到。
    gxIns.noSel=function(ev){ev.preventDefault();};document.addEventListener('selectstart',gxIns.noSel,true);document.addEventListener('dragstart',gxIns.noSel,true);
    gxHudShow('套索开 · 拖拽画圈 或 单击元素');
    // ★贴边元素★:鼠标滑出视口时坐标钳回边界,套索点贴边不丢线。
    function gxClampX(v){return v<0?0:(v>window.innerWidth?window.innerWidth:v);}
    function gxClampY(v){return v<0?0:(v>window.innerHeight?window.innerHeight:v);}
    // 收尾抽成函数:mouseup 与"窗外松开后回窗内(buttons===0)"共用。Phase1 选完不退出套索态(保留 overlay,可连选)。
    gxIns.finish=function(e){
      var was=gxIns.draw,pts=gxIns.pts;gxIns.draw=false;gxIns.pts=[];
      if(gxIns.ctx)gxIns.ctx.clearRect(0,0,cv.width,cv.height);
      var fx=gxClampX(e.clientX),fy=gxClampY(e.clientY);
      var els,rect;
      // 几乎没动(点选)或路径太短 → 单元素;否则按不规则套索多边形选质心在内的元素。
      if(!was||pts.length<4){var one=gxAt(fx,fy);els=one?[one]:[];rect=one?gxRect(one):{x:fx,y:fy,w:0,h:0};}
      else{els=gxInLasso(pts);if(els.length===0){var fcx=0,fcy=0;for(var fk=0;fk<pts.length;fk++){fcx+=pts[fk].x;fcy+=pts[fk].y;}var fb=gxAt(fcx/pts.length,fcy/pts.length);if(fb)els=[fb];}var mnx=1e9,mny=1e9,mxx=-1e9,mxy=-1e9;for(var k=0;k<pts.length;k++){var p=pts[k];if(p.x<mnx)mnx=p.x;if(p.x>mxx)mxx=p.x;if(p.y<mny)mny=p.y;if(p.y>mxy)mxy=p.y;}rect={x:Math.round(mnx),y:Math.round(mny),w:Math.round(mxx-mnx),h:Math.round(mxy-mny)};}
      try{if(window.getSelection)window.getSelection().removeAllRanges();}catch(ge){}
      var snap=[];for(var i=0;i<els.length;i++)snap.push(gxSnap(els[i]));
      gxHudShow('选中 '+snap.length+' 个'+(snap[0]?' · '+snap[0].selector:' ·(空,圈大点或单击)'));gxMarkSel(els);
      if(snap.length)parent.postMessage({__gx:true,type:'inspect-select',selection:{rect:rect,elements:snap}},'*');
    };
    // 静默:不做 hover 高亮,只画套索线(用户要求"只看到套索区域")。
    // ★move/up 挂 window(捕获)而非 overlay★:鼠标拖出窗口边缘仍能追踪;窗外松开漏掉的 mouseup 由 buttons===0 补收尾。
    gxIns.onMove=function(e){
      if(!gxIns.draw)return;
      if(e.buttons===0){gxIns.finish(e);return;}
      gxIns.pts.push({x:gxClampX(e.clientX),y:gxClampY(e.clientY)});gxRedraw();gxHudShow('画圈中 · 点 '+gxIns.pts.length);
    };
    gxIns.onUp=function(e){if(!gxIns.draw)return;gxIns.finish(e);};
    ov.addEventListener('mousedown',function(e){gxIns.draw=true;gxIns.pts=[{x:gxClampX(e.clientX),y:gxClampY(e.clientY)}];gxClrHi();e.preventDefault();});
    window.addEventListener('mousemove',gxIns.onMove,true);
    window.addEventListener('mouseup',gxIns.onUp,true);
  }
  function gxStop(){if(!gxIns.on)return;gxIns.on=false;gxClrHi();gxHudHide();if(gxIns.noSel){document.removeEventListener('selectstart',gxIns.noSel,true);document.removeEventListener('dragstart',gxIns.noSel,true);gxIns.noSel=null;}if(gxIns.onMove){window.removeEventListener('mousemove',gxIns.onMove,true);gxIns.onMove=null;}if(gxIns.onUp){window.removeEventListener('mouseup',gxIns.onUp,true);gxIns.onUp=null;}gxIns.finish=null;if(gxIns.ov)gxIns.ov.remove();if(gxIns.cv)gxIns.cv.remove();gxIns.ov=null;gxIns.cv=null;gxIns.ctx=null;gxIns.draw=false;gxIns.pts=[];}
  // 造物灵魂:LLM→造物指令(父页发 {__gxcmd:'command'})→ 调造物注册的 window.gxOnCommand(LLM 是使用者,发指令不重画)。
  // + 造物自检(Phase 4):父页发 {__gxcmd:'selftest'} → 枚举全部 data-gx-callback 回报清单(声明式 → 可自动枚举)。
  window.addEventListener('message',function(e){
    if(!e.data) return;
    if(e.data.__gxcmd==='command'){
      if(typeof window.gxOnCommand==='function'){ try{ window.gxOnCommand(e.data.command); }catch(err){} }
      return;
    }
    if(e.data.__gxcmd==='inspect'){ if(e.data.enable){ gxStart(); } else { gxStop(); } return; }
    if(e.data.__gxcmd!=='selftest') return;
    var seen={}, cbs=[];
    var els=document.querySelectorAll('[data-gx-callback]');
    for(var i=0;i<els.length;i++){
      var el=els[i], id=el.getAttribute('data-gx-callback');
      if(id&&!seen[id]){ seen[id]=true; cbs.push({callbackId:id,realtime:el.hasAttribute('data-gx-realtime'),tag:(el.tagName||'').toLowerCase(),source:'dom'}); }
    }
    // ★程序化回调(canvas/JS 驱动)★:经 window.gx.emit/register 声明的也算"已接通的感知点"——
    // 否则 canvas 游戏(整块画布一个元素,无法挂 data-gx-callback)会被误报"无回调",让 AI 以为没接上。
    for(var k in gxRegistered){ if(gxRegistered.hasOwnProperty(k)&&!seen[k]){ seen[k]=true; cbs.push({callbackId:k,realtime:false,tag:'js',source:'js'}); } }
    // ★模拟 AI 操作通道(§7)★:硬验证 LLM→造物 的指令入口 window.gxOnCommand 是否真注册了。
    // 没注册 = AI 的落子等指令到不了造物(真机 bug:造了棋盘却没接 gxOnCommand,AI 落子无门)。
    parent.postMessage({__gx:true,type:'selftest-report',callbacks:cbs,gxOnCommand:(typeof window.gxOnCommand==='function')},'*');
  });
})();<\/script>`;

// AI 经 render_artifact 往返调用:注入 gx 引导 + 灌入 HTML 到**常驻 iframe**(草稿渲染)。
// ★不立即展示★(用户 2026-06-04"做好了才展示"):中间多次 render 只更新草稿、iframe 隐藏渲染
// (可 selftest),回合结束由 presentArtifactIfReady 展示最终态 → 用户不被"一边改一边弹"打断。
export function renderArtifact(html: string, showChrome = true): void {
  setArtifactHtml(GX_BOOTSTRAP + html);
  setArtifactShowChrome(showChrome);
  setArtifactRendered(true);
  // 新文档:旧选择态作废(srcdoc 替换后旧 overlay 随之销毁)。
  setArtifactInspect(false);
  setInspectSel(null);
}

// ★造物灵魂:LLM→造物指令★(AI 经 artifact_command 往返调用)。转发到造物 iframe → 其 window.gxOnCommand 执行。
// AI 是使用者:发结构化指令(如落白子),造物自身 JS 本地执行,不重画整个造物。
export function commandArtifact(command: string): boolean {
  const win = artifactIframe?.contentWindow;
  if (!win) return false;
  win.postMessage({ __gxcmd: "command", command }, "*");
  return true;
}

// ★造物思考窗(0-OPUS34 续点)★:造物回合(如五子棋落子触发的回合)进行时,把 AI 的实时 reasoning
// 推到造物上的一个思考浮层 —— 否则用户盯着静止的棋盘干等几十秒、不知道 AI 在想什么(自我感知/在场感)。
// 与主聊天的"思考过程"折叠是同一份 reasoning 流,只是消费面不同(用户在看造物、不在看聊天)。
// chat.ts 的 artifactReceiveRound 在收到 thinking chunk 时喂这里;回合结束清空。
export const [artifactThinking, setArtifactThinking] = createSignal<string>("");
export const [artifactThinkingActive, setArtifactThinkingActive] = createSignal(false);
/// 回合开始:开思考窗(清旧文)。
export function beginArtifactThinking(): void {
  setArtifactThinking("");
  setArtifactThinkingActive(true);
}
/// 累积一段 reasoning 到思考窗(只留尾部若干字,够看"正在想什么"即可,不堆历史)。
export function pushArtifactThinking(delta: string): void {
  if (!delta) return;
  setArtifactThinkingActive(true);
  setArtifactThinking((prev) => {
    const next = prev + delta;
    return next.length > 600 ? next.slice(next.length - 600) : next;
  });
}
/// 回合结束:关思考窗(留文以便淡出由组件处理;这里只置非活跃)。
export function endArtifactThinking(): void {
  setArtifactThinkingActive(false);
}

// 造物覆盖层(流3)= AI 在造物里的常驻"嘴":右上角槽,推主动提示/建议/吐槽。
// 可信 SolidJS 层、浮在沙箱内容之上(不进沙箱)。AI 经 push_artifact_notice 往返调用。
export const [artifactNotice, setArtifactNotice] = createSignal<string>("");
// 悬浮窗自动消失计时器(用户:别一直挂在那里)。新提示来 / 手动关 时重置。
let noticeTimer: ReturnType<typeof setTimeout> | undefined;
/// 关闭悬浮提示并清计时器(× 按钮 / 自动消失共用)。
export function dismissArtifactNotice(): void {
  if (noticeTimer) {
    clearTimeout(noticeTimer);
    noticeTimer = undefined;
  }
  setArtifactNotice("");
}
export function pushArtifactNotice(text: string): void {
  setArtifactNotice(text);
  // 覆盖层 = AI 主动给用户的提示/建议,本就要被看到 → 立即展示(若已渲染过造物)。
  setArtifactRendered(true);
  setArtifactCanvasOpen(true);
  // 一段时间后自动消失(防永久占屏);按内容长短给阅读时间,封顶。新提示进来上面已重置文本,这里重置计时。
  if (noticeTimer) clearTimeout(noticeTimer);
  const ms = Math.min(8000 + text.length * 80, 20000);
  noticeTimer = setTimeout(() => {
    setArtifactNotice("");
    noticeTimer = undefined;
  }, ms);
}

// 自检结果:声明的回调面 + AI 指令通道 gxOnCommand 是否注册(§7 硬验证)。
export interface SelftestResult {
  callbacks: unknown[];
  gxOnCommand: boolean;
}
// 当前造物 iframe 引用 + 自检回报的待决 resolver(模块级:供导出的 runArtifactSelftest 访问)。
let artifactIframe: HTMLIFrameElement | undefined;
let selftestResolve: ((r: SelftestResult) => void) | null = null;

// 造物自检(Phase 4 + §7):令 iframe 枚举回调面 + 检测 gxOnCommand 指令通道(1.5s 超时兜底:空+通道未通)。
export function runArtifactSelftest(): Promise<SelftestResult> {
  return new Promise((resolve) => {
    const win = artifactIframe?.contentWindow;
    if (!win) {
      resolve({ callbacks: [], gxOnCommand: false });
      return;
    }
    selftestResolve = resolve;
    win.postMessage({ __gxcmd: "selftest" }, "*");
    setTimeout(() => {
      if (selftestResolve === resolve) {
        selftestResolve = null;
        resolve({ callbacks: [], gxOnCommand: false });
      }
    }, 1500);
  });
}

// ── 网页调试:套索/点选检视(计划/网页调试窗-可视化框选改源.md Phase 1)──
// 选择态由横栏「套索」按钮切换;命令经 postMessage 进 iframe 的 gx 调试运行时(GX_BOOTSTRAP)。
export const [artifactInspect, setArtifactInspect] = createSignal(false);
export interface InspectElement {
  selector: string;
  tag: string;
  id: string;
  classes: string;
  text: string;
  outerHTML: string;
  rect: { x: number; y: number; w: number; h: number };
}
export interface InspectSelection {
  rect: { x: number; y: number; w: number; h: number };
  elements: InspectElement[];
}
// 当前框选结果(iframe 回传);非空时展示"修改建议"输入面板。
const [inspectSel, setInspectSel] = createSignal<InspectSelection | null>(null);

// 切套索选择态:postMessage 令 iframe 调试运行时进/出选择态 + 同步前端按钮态。关时清当前框选。
export function setArtifactInspectMode(enable: boolean): void {
  const win = artifactIframe?.contentWindow;
  if (win) win.postMessage({ __gxcmd: "inspect", enable }, "*");
  setArtifactInspect(enable);
  if (!enable) setInspectSel(null);
}

const ArtifactCanvas: Component = () => {
  // ★窗口拖动(用户 2026-06-04:顶部横栏硬性可拖)★:横栏 mousedown 拖动浮窗。
  // pos=null 时用默认居中 CSS;拖动后变 left/top 绝对位置。
  const [pos, setPos] = createSignal<{ x: number; y: number } | null>(null);
  let panelRef: HTMLDivElement | undefined;
  let dragFrom: { mx: number; my: number; x: number; y: number } | null = null;
  const onDragMove = (e: MouseEvent) => {
    if (!dragFrom) return;
    setPos({ x: dragFrom.x + (e.clientX - dragFrom.mx), y: dragFrom.y + (e.clientY - dragFrom.my) });
  };
  const onDragEnd = () => {
    dragFrom = null;
    window.removeEventListener("mousemove", onDragMove);
    window.removeEventListener("mouseup", onDragEnd);
  };
  const onTitleDown = (e: MouseEvent) => {
    const cur = pos() ?? (panelRef ? { x: panelRef.offsetLeft, y: panelRef.offsetTop } : { x: 0, y: 0 });
    dragFrom = { mx: e.clientX, my: e.clientY, x: cur.x, y: cur.y };
    window.addEventListener("mousemove", onDragMove);
    window.addEventListener("mouseup", onDragEnd);
    e.preventDefault();
  };
  onCleanup(() => onDragEnd());
  // ★网页调试 Phase 1:提交框选 + 修改建议★ —— 拼成一条带 DOM 上下文的消息走普通回合(doSend),
  // AI 在它渲染该造物用的 HTML 源码里定位框选元素、按建议改、render_artifact 重渲。
  const [suggestion, setSuggestion] = createSignal("");
  const submitVisualEdit = async () => {
    const sel = inspectSel();
    const sug = suggestion().trim();
    if (!sel || !sug) return;
    const lines = sel.elements
      .map((el, i) => `${i + 1}. ${el.selector}${el.text ? `  文字:"${el.text}"` : ""}`)
      .join("\n");
    const html = sel.elements.map((el) => el.outerHTML).join("\n");
    const payload =
      `${sug}\n\n[网页调试·套索] 我在造物画布里用套索选中了下面的元素。` +
      `源 HTML = 你上一次 render_artifact 传入的那整段(就在本对话里,直接据它改;不要去磁盘找文件、不要用文件/MCP 工具)。` +
      `请只修改选中的元素,不要改动未选中的部分,改完用 render_artifact 重渲整段 HTML(canvas_id 用 "main"):\n${lines}\n\n` +
      `选中元素的 outerHTML:\n${html}`;
    setInspectSel(null);
    setSuggestion("");
    setArtifactInspectMode(false);
    await doSend(payload);
  };
  // 监听造物 iframe 的 gx 回传:event(流2)→ artifact_event;selftest-report(Phase 4)→ 解析自检 promise。
  onMount(() => {
    const onMsg = (e: MessageEvent) => {
      const d = e.data as {
        __gx?: boolean;
        type?: string;
        callbackId?: string;
        value?: unknown;
        realtime?: boolean;
        callbacks?: unknown[];
        gxOnCommand?: boolean;
        selection?: InspectSelection;
      };
      if (!d || d.__gx !== true) return;
      // 来源校验:只收当前造物 iframe 的消息(沙箱 null origin,比对 contentWindow)。
      if (artifactIframe && e.source !== artifactIframe.contentWindow) return;
      if (d.type === "event") {
        // 造物交互 v2 §1:跑"接收回合"——写造物(render/selftest)进度进聊天(懒气泡),
        // 端口落子静默;回合结束 present 最终态(做好了才展示)。realtime 高频输入不进聊天。
        void artifactReceiveRound("main", String(d.callbackId ?? ""), String(d.value ?? ""), !!d.realtime)
          .catch(() => {});
      } else if (d.type === "selftest-report" && selftestResolve) {
        const r = selftestResolve;
        selftestResolve = null;
        r({ callbacks: Array.isArray(d.callbacks) ? d.callbacks : [], gxOnCommand: !!d.gxOnCommand });
      } else if (d.type === "inspect-select") {
        // 网页调试 Phase 1:iframe 回传框选结果 → 展示"修改建议"输入面板。
        if (d.selection && Array.isArray(d.selection.elements) && d.selection.elements.length > 0) {
          setInspectSel(d.selection);
        }
      }
    };
    window.addEventListener("message", onMsg);
    onCleanup(() => window.removeEventListener("message", onMsg));
  });
  return (
    // ★可拖动浮窗(非全屏遮罩)★:iframe 在"已渲染"时常驻 DOM(草稿期也在,可执行脚本+selftest);
    // 对用户的可见性由 artifactCanvasOpen 控制(display);位置由 pos 控制(横栏拖动);无横栏(桌宠)则整窗可拖。
    <Show when={artifactRendered()}>
      <div
        ref={panelRef}
        style={{
          position: "fixed",
          left: pos() ? `${pos()!.x}px` : "max(8px, calc(100vw - 760px))",
          top: pos() ? `${pos()!.y}px` : "72px",
          "z-index": "900",
          width: "min(720px, 92vw)",
          height: "min(600px, 82vh)",
          background: "#1b1d22",
          "border-radius": "12px",
          overflow: "hidden",
          display: artifactCanvasOpen() ? "flex" : "none",
          "flex-direction": "column",
          "box-shadow": "0 12px 48px rgba(0,0,0,0.5)",
          border: "1px solid #2e323a",
        }}
      >
        <Show
          when={artifactShowChrome()}
          fallback={
            // 无横栏(桌宠等):窗体本身可拖(顶部 24px 透明拖动条),不占视觉。
            <div onMouseDown={onTitleDown} style={{ position: "absolute", top: "0", left: "0", right: "0", height: "24px", cursor: "move", "z-index": "5" }} />
          }
        >
          <div
            onMouseDown={onTitleDown}
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              padding: "8px 12px",
              background: "#23262d",
              color: "#e6e6e6",
              "font-size": "13px",
              "border-bottom": "1px solid #2e323a",
              cursor: "move",
              "user-select": "none",
            }}
          >
            <span>造物画布</span>
            <div style={{ display: "flex", "align-items": "center", gap: "4px" }}>
              <button
                onClick={() => setArtifactInspectMode(!artifactInspect())}
                onMouseDown={(e) => e.stopPropagation()}
                title={artifactInspect() ? "退出框选检视" : "套索:框选元素,提改进让 AI 改源"}
                style={{
                  background: artifactInspect() ? "#0a84ff" : "transparent",
                  border: "none",
                  color: artifactInspect() ? "#fff" : "#9aa0aa",
                  "font-size": "13px",
                  cursor: "pointer",
                  "border-radius": "6px",
                  padding: "2px 7px",
                  "line-height": "1.4",
                }}
              >
                ⌖ 套索
              </button>
              <button
                onClick={userCloseArtifact}
                onMouseDown={(e) => e.stopPropagation()}
                title="关闭"
                style={{
                  background: "transparent",
                  border: "none",
                  color: "#9aa0aa",
                  "font-size": "18px",
                  cursor: "pointer",
                  "line-height": "1",
                }}
              >
                ×
              </button>
            </div>
          </div>
        </Show>
          <iframe
            ref={(el) => (artifactIframe = el)}
            title="artifact"
            sandbox="allow-scripts"
            srcdoc={artifactHtml()}
            style={{ flex: "1", width: "100%", border: "none", background: "#fff" }}
          />
          {/* ★造物思考窗★:回合进行时左下角浮层显示 AI 实时 reasoning(用户不必干等静止的造物)。 */}
          <Show when={artifactThinkingActive()}>
            <div
              style={{
                position: "absolute",
                bottom: "12px",
                left: "12px",
                "max-width": "70%",
                "max-height": "38%",
                "z-index": "9",
                display: "flex",
                "flex-direction": "column",
                gap: "4px",
                padding: "8px 11px",
                background: "rgba(20,22,27,0.92)",
                color: "#c9cdd6",
                "border-radius": "10px",
                "font-size": "11.5px",
                "line-height": "1.55",
                border: "1px solid #34384180",
                "box-shadow": "0 6px 20px rgba(0,0,0,0.4)",
                "backdrop-filter": "blur(2px)",
              }}
            >
              <div style={{ display: "flex", "align-items": "center", gap: "6px", color: "#8a909a", "font-size": "11px" }}>
                <span class="artifact-think-dot" style={{ width: "6px", height: "6px", "border-radius": "50%", background: "#0a84ff", display: "inline-block" }} />
                <span>正在思考…</span>
              </div>
              <div style={{ overflow: "hidden", "white-space": "pre-wrap", "text-overflow": "ellipsis", opacity: "0.85" }}>
                {artifactThinking()}
              </div>
            </div>
          </Show>
          <Show when={artifactNotice()}>
            <div
              style={{
                position: "absolute",
                top: "44px",
                right: "12px",
                "max-width": "62%",
                "z-index": "10",
                display: "flex",
                "align-items": "flex-start",
                gap: "8px",
                padding: "8px 10px",
                background: "#0a84ff",
                color: "#fff",
                "border-radius": "10px",
                "font-size": "12.5px",
                "line-height": "1.5",
                "box-shadow": "0 6px 20px rgba(10,132,255,0.4)",
              }}
            >
              <span style={{ flex: "1", "white-space": "pre-wrap" }}>{artifactNotice()}</span>
              <button
                onClick={() => void navigator.clipboard?.writeText(artifactNotice()).catch(() => {})}
                title="复制"
                style={{
                  background: "transparent",
                  border: "none",
                  color: "rgba(255,255,255,0.85)",
                  cursor: "pointer",
                  "font-size": "12px",
                  "line-height": "1",
                  padding: "0 2px",
                }}
              >
                ⧉
              </button>
              <button
                onClick={dismissArtifactNotice}
                title="知道了"
                style={{
                  background: "transparent",
                  border: "none",
                  color: "rgba(255,255,255,0.85)",
                  cursor: "pointer",
                  "font-size": "14px",
                  "line-height": "1",
                }}
              >
                ×
              </button>
            </div>
          </Show>
          <Show when={inspectSel()}>
            <div
              style={{
                position: "absolute",
                left: "12px",
                right: "12px",
                bottom: "12px",
                "z-index": "12",
                background: "#23262d",
                border: "1px solid #0a84ff",
                "border-radius": "10px",
                padding: "10px",
                display: "flex",
                "flex-direction": "column",
                gap: "8px",
                "box-shadow": "0 8px 28px rgba(0,0,0,0.55)",
              }}
            >
              <div style={{ "font-size": "12px", color: "#9aa0aa", "line-height": "1.5" }}>
                已框选 {inspectSel()!.elements.length} 个元素:
                {inspectSel()!.elements.map((el) => el.selector).slice(0, 3).join("  /  ")}
              </div>
              <textarea
                value={suggestion()}
                onInput={(e) => setSuggestion(e.currentTarget.value)}
                placeholder="描述要怎么改(例:把这个按钮改成绿色、字号调大)…"
                rows={2}
                style={{
                  width: "100%",
                  resize: "vertical",
                  background: "#1b1d22",
                  color: "#e6e6e6",
                  border: "1px solid #2e323a",
                  "border-radius": "6px",
                  padding: "6px 8px",
                  "font-size": "12.5px",
                  "box-sizing": "border-box",
                }}
              />
              <div style={{ display: "flex", "justify-content": "flex-end", gap: "8px" }}>
                <button
                  onClick={() => {
                    setInspectSel(null);
                    setSuggestion("");
                  }}
                  style={{
                    background: "transparent",
                    border: "1px solid #3a3f48",
                    color: "#9aa0aa",
                    "border-radius": "6px",
                    padding: "4px 12px",
                    "font-size": "12.5px",
                    cursor: "pointer",
                  }}
                >
                  取消
                </button>
                <button
                  onClick={() => void submitVisualEdit()}
                  disabled={!suggestion().trim()}
                  style={{
                    background: suggestion().trim() ? "#0a84ff" : "#2e323a",
                    border: "none",
                    color: "#fff",
                    "border-radius": "6px",
                    padding: "4px 12px",
                    "font-size": "12.5px",
                    cursor: suggestion().trim() ? "pointer" : "default",
                  }}
                >
                  发送给 AI
                </button>
              </div>
            </div>
          </Show>
        </div>
    </Show>
  );
};

export default ArtifactCanvas;
