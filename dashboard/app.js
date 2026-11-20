"use strict";
const byId=id=>document.getElementById(id);
let data=null,csrf="",busy=false,previous=null,measuredFps=null,preview=null,toastTimer=null;
const actionButtons=[...document.querySelectorAll("[data-action]")];
function toast(message){byId("toast").textContent=message;byId("toast").hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>byId("toast").hidden=true,2800);}
function duration(seconds){seconds=Math.max(0,Math.floor(seconds||0));const h=Math.floor(seconds/3600),m=Math.floor(seconds%3600/60),s=seconds%60;return h?[h,m,s].map(x=>String(x).padStart(2,"0")).join(":"):[m,s].map(x=>String(x).padStart(2,"0")).join(":");}
function buttons(){const enabled=Boolean(data?.control?.enabled);byId("start").disabled=busy||!csrf||enabled;byId("stop").disabled=busy||!csrf||!enabled;byId("restart").disabled=busy||!csrf;}
function render(value){
 data=value;csrf=value.csrf_token||"";
 const receiver=value.receiver||{},control=value.control||{},details=receiver.details||{},media=value.media||{};
 const age=Math.max(0,(value.now||Date.now()/1000)-(receiver.updated_at||0));
 let title,description,tone="pending";const revisionMatch=details.control_revision===control.revision;
 if(!revisionMatch){title=control.enabled?"正在开启 / 重新连接":"正在停止串流";description="正在完成会话切换，请稍候。";}
 else if(receiver.state==="paused"){title="串流已停止";description="自动重连已暂停。点击“开启串流”后重新接收画面。";tone="idle";}
 else if(age>(receiver.state==="connecting"?180:receiver.state==="waiting_console"?90:35)){title="接收状态未更新";description="管理页面可访问，但接收进程状态已过期。可尝试重新连接。";tone="error";}
 else if(receiver.state==="streaming"&&media.active){title="正在接收 · 正在分发";description="XSX 画面已进入局域网分发服务，可在直播伴侣拉流。";tone="live";}
 else if(receiver.state==="waiting_console"){title="等待 XSX 开机";description="开启 Xbox 并登录游戏账号，接收器会继续检查；不会自动唤醒主机。";}
 else if(receiver.state==="retrying"){title="正在等待重连";description="当前连接已断开，接收器会自动重试。也可以点击“重新连接 XSX”。";}
 else if(receiver.state==="connecting"){title="正在连接 XSX";description="正在建立微软授权会话与局域网媒体连接。";}
 else {title="正在准备画面";description="接收器正在等待可分发的音视频。";}
 byId("stateTitle").textContent=title;byId("stateBadge").className="badge "+tone;byId("stateDetail").textContent=description;
 byId("lastSync").textContent="最后刷新 "+new Date().toLocaleTimeString("zh-CN",{hour12:false});
 const streaming=receiver.state==="streaming"&&control.enabled&&revisionMatch&&age<35;
 const video=media.video||{};
 byId("resolution").textContent=streaming&&video.width?video.width+" × "+video.height:"—";
 if(streaming){
  if(previous&&previous.revision===control.revision&&details.elapsed_seconds>previous.elapsed){measuredFps=(details.video_frames-previous.frames)/(details.elapsed_seconds-previous.elapsed);}
  if(!previous||previous.revision!==control.revision){measuredFps=null;}
  if(!previous||details.elapsed_seconds!==previous.elapsed){previous={revision:control.revision,elapsed:details.elapsed_seconds,frames:details.video_frames};}
  byId("fps").textContent=measuredFps!==null?measuredFps.toFixed(1)+" fps":"统计中";
  byId("bitrate").textContent=media.kbps>0?(media.kbps/1000).toFixed(1)+" Mb/s":"统计中";
  byId("uptime").textContent=duration(details.elapsed_seconds);
 }else{previous=null;measuredFps=null;for(const id of ["fps","bitrate","uptime"])byId(id).textContent="—";}
 for(const key of ["browser","flv","rtmp","dashboard"]){const input=byId("link-"+key);if(value.links?.[key])input.value=value.links[key];}
 if(value.links?.browser)byId("openBrowser").href=value.links.browser;
 buttons();
}
async function request(path,options={}){
 const abort=new AbortController(),timer=setTimeout(()=>abort.abort(),6000);
 try{const response=await fetch(path,{cache:"no-store",credentials:"same-origin",...options,signal:abort.signal});const body=await response.json();if(!response.ok)throw new Error(body.error||"请求失败");return body;}finally{clearTimeout(timer);}
}
async function refresh(){try{render(await request("/api/status"));}catch(e){csrf="";buttons();byId("stateTitle").textContent="管理服务连接中";byId("stateBadge").className="badge error";byId("stateDetail").textContent="暂时无法读取状态，将自动重试。已有串流不受此页面断线影响。";}}
async function poll(){await refresh();setTimeout(poll,2000);}
for(const button of actionButtons)button.addEventListener("click",async()=>{
 if(busy||!csrf)return;busy=true;buttons();
 try{
  const action=button.dataset.action;
  await request("/api/control",{method:"POST",headers:{"Content-Type":"application/json","X-XSX-Token":csrf},body:JSON.stringify({action})});
  toast(action==="start"?"已开启接收，正在连接 XSX":action==="stop"?"正在停止接收，开关状态已保存":"正在重新连接 XSX");
  await refresh();
 }catch(e){toast(e.message||"操作失败，请刷新后重试");}
 finally{busy=false;buttons();}
});
for(const button of document.querySelectorAll("[data-copy]"))button.addEventListener("click",async()=>{
 const input=byId("link-"+button.dataset.copy);if(!input.value){toast("地址尚未加载");return;}
 let copied=false;
 if(window.isSecureContext&&navigator.clipboard){try{await navigator.clipboard.writeText(input.value);copied=true;}catch(e){}}
 if(!copied){input.focus();input.select();input.setSelectionRange(0,input.value.length);try{copied=document.execCommand("copy");}catch(e){}}
 if(copied){button.textContent="已复制";toast("地址已复制");setTimeout(()=>button.textContent="复制",1800);}else{toast("已选中地址，请长按或按 Ctrl+C 复制");}
});
byId("previewToggle").addEventListener("click",()=>{
 if(preview){preview.remove();preview=null;byId("previewPlaceholder").hidden=false;byId("previewToggle").textContent="打开预览";return;}
 if(!data?.links?.preview){toast("预览地址尚未加载");return;}
 preview=document.createElement("iframe");preview.title="XSX 静音画面预览";preview.allow="autoplay; fullscreen";preview.src=data.links.preview;byId("previewPlaceholder").hidden=true;byId("previewArea").append(preview);byId("previewToggle").textContent="关闭预览";
});
poll();
