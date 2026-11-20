// Headless LAN-only encoded-media receiver. Upstream references are pinned in UPSTREAM_COMMIT.
use anyhow::{anyhow,bail,Context,Result};
use std::{net::{IpAddr,SocketAddr,UdpSocket as StdUdpSocket},time::{Duration,Instant}};
use str0m::{Rtc,Candidate,Event,Input,Output};
use str0m::channel::{ChannelId,ChannelConfig};
use str0m::change::SdpAnswer;
use str0m::media::{Direction,MediaKind};
use str0m::net::{Protocol,Receive};
use tokio::net::UdpSocket;
use xsx_native::relay::MediaRelay;
use xsx_native::{auth::XboxAuth,xhome::XHomeClient,rtc::protocol::{self,InboundMsg}};
const CHANNELS:[(&str,&str);4]=[("chat","chatV1"),("control","controlV1"),("message","messageV1"),("input","1.0")];
fn uuid()->String{
 let mut b=[0u8;16];getrandom::fill(&mut b).expect("OS randomness");b[6]=(b[6]&15)|64;b[8]=(b[8]&63)|128;
 format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15])
}
fn write_dc(rtc:&mut Rtc,id:Option<ChannelId>,v:&impl serde::Serialize){
 if let Some(id)=id {if let Some(mut ch)=rtc.channel(id) {let _=ch.write(true,&serde_json::to_vec(v).expect("protocol serialization"));}}
}
#[tokio::main]
async fn main()->Result<()> {
 tracing_subscriber::fmt().with_ansi(false).with_env_filter("warn").init();
 match std::env::args().nth(1).as_deref() {
  Some("--healthcheck")=>return xsx_native::health::check(),
  Some("--auth-check")=>{
   let _=XboxAuth::new().get_auth_header().await.context("Authorization check failed")?;
   println!("AUTH_CHECK valid=true");return Ok(());
  },
  Some(_)=>bail!("Unknown option"),None=>{}
 }
 let (shutdown_tx,mut shutdown)=tokio::sync::watch::channel(false);
 tokio::spawn(async move {
  let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
  tokio::select!{_=term.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
  let _=shutdown_tx.send(true);
 });
 let initial=xsx_native::control::ControlState::load().context("Control state")?;
 let (control_tx,mut control)=tokio::sync::watch::channel(initial);
 xsx_native::dashboard::start(control_tx).await?;
 loop {
  if *shutdown.borrow(){break;}
  let desired=control.borrow_and_update().clone();
  if !desired.enabled {
   xsx_native::health::update("paused",serde_json::json!({"control_revision":desired.revision}))?;
   tokio::select!{
    _=tokio::time::sleep(Duration::from_secs(20))=>{},
    _=shutdown.changed()=>{},
    _=control.changed()=>{}
   }
   continue;
  }
  xsx_native::health::update("connecting",serde_json::json!({"control_revision":desired.revision}))?;
  let outcome=run_session(shutdown.clone(),control.clone(),desired.revision).await;
  if *shutdown.borrow(){break;}
  if control.borrow().revision!=desired.revision {continue;}
  let offline=outcome.as_ref().err().is_some_and(|e|e.to_string()=="CONSOLE_OFFLINE");
  let delay=if offline{60}else{10};
  if let Err(e)=outcome {println!("SESSION_RETRY delay_seconds={delay} reason={e}");}
  xsx_native::health::update(if offline{"waiting_console"}else{"retrying"},serde_json::json!({"retry_seconds":delay,"control_revision":desired.revision}))?;
  tokio::select!{
   _=tokio::time::sleep(Duration::from_secs(delay))=>{},
   _=shutdown.changed()=>{},
   _=control.changed()=>{}
  }
 }
 xsx_native::health::update("stopped",serde_json::json!({}))?;
 println!("STOPPED");Ok(())
}
async fn run_session(mut shutdown:tokio::sync::watch::Receiver<bool>,mut control:tokio::sync::watch::Receiver<xsx_native::control::ControlState>,revision:u64)->Result<()> {
 let setup=async {
  let auth=XboxAuth::new();if !auth.load_cached_tokens().await.context("Authorization unavailable")?{bail!("AUTH_REQUIRED");}
  let mut client=XHomeClient::new(auth);client.login().await.context("xHome login")?;
  let consoles=client.get_consoles().await.context("console discovery")?;
  let configured=std::env::var("XBOX_SERVER_ID").ok();
  let console=consoles.iter().find(|c|
   c.power_state.eq_ignore_ascii_case("On")&&configured.as_ref().is_none_or(|id|id==&c.server_id)
  ).ok_or_else(||anyhow!("CONSOLE_OFFLINE"))?;
  println!("CONSOLE selected=true power_state={}",console.power_state);
  let cfg=client.create_session(&console.server_id,Some(&console.play_path)).await.context("session setup")?;
  Ok::<_,anyhow::Error>((client,cfg))
 };
 let (client,cfg)=tokio::select! {
  value=tokio::time::timeout(Duration::from_secs(170),setup)=>value.context("Session setup timed out")??,
  _=shutdown.changed()=>return Ok(()),
  _=control.changed()=>return Ok(())
 };
 let pulse_client=client.clone();let pulse_path=cfg.session_path.clone();
 let pulse_seconds=(cfg.keep_alive_pulse_seconds.unwrap_or(30)as u64/2).clamp(10,20);
 let keep_task=tokio::spawn(async move {
  loop {
   tokio::time::sleep(Duration::from_secs(pulse_seconds)).await;
   match pulse_client.send_keepalive(&pulse_path).await {
    Ok(_)=>{},
    Err(e)=>{if e.to_string().contains("400"){println!("API_KEEPALIVE state=not_needed");break;}
     println!("API_KEEPALIVE state=retry");}
   }
  }
 });
 let result=tokio::select! {
  value=receive(&client,&cfg.session_path,revision)=>value,
  _=shutdown.changed()=>{println!("STOP_REQUESTED");Ok(())},
  _=control.changed()=>{println!("CONTROL_SESSION_END");Ok(())}
 };
 keep_task.abort();let _=keep_task.await;
 match tokio::time::timeout(Duration::from_secs(8),client.delete_session(&cfg.session_path)).await{
  Ok(Ok(_))=>println!("SESSION_CLOSED"),_=>println!("SESSION_CLOSE pending_expiry=true")
 }
 result
}
async fn receive(client:&XHomeClient,path:&str,revision:u64)->Result<()>{
 let xbox_ip:IpAddr=std::env::var("XBOX_IP").unwrap_or_else(|_|"192.168.1.180".into()).parse()?;
 let probe=StdUdpSocket::bind("0.0.0.0:0")?;probe.connect(SocketAddr::new(xbox_ip,9002))?;let local_ip=probe.local_addr()?.ip();drop(probe);
 let udp=UdpSocket::bind(SocketAddr::new(local_ip,0)).await?;let local=udp.local_addr()?;
 let mut rtc=Rtc::new(Instant::now());rtc.add_local_candidate(Candidate::host(local,"udp").map_err(|e|anyhow!("{e}"))?);
 let mut control=None;let mut message=None;let mut input=None;
 let (video_mid,offer,pending)={
  let mut change=rtc.sdp_api();
  for(label,proto)in CHANNELS{
   let id=change.add_channel_with_config(ChannelConfig{label:label.into(),ordered:true,protocol:proto.into(),negotiated:None,..Default::default()});
   if label=="control"{control=Some(id)}if label=="message"{message=Some(id)}if label=="input"{input=Some(id)}
  }
  let mid=change.add_media(MediaKind::Video,Direction::RecvOnly,None,None,None);
  change.add_media(MediaKind::Audio,Direction::SendRecv,None,None,None);
  let(offer,p)=change.apply().ok_or_else(||anyhow!("No SDP offer"))?;(mid,offer.to_sdp_string(),p)
 };
 let answer=client.exchange_sdp_offer(path,&offer).await.context("SDP exchange")?;
 println!("SDP dtls={} sdes={}",answer.contains("a=fingerprint:"),answer.contains("a=crypto:"));
 let filtered=answer.lines().filter(|l|!l.starts_with("a=candidate:")).collect::<Vec<_>>().join("\r\n")+"\r\n";
 rtc.sdp_api().accept_answer(pending,SdpAnswer::from_sdp_string(&filtered).map_err(|e|anyhow!("{e}"))?).map_err(|e|anyhow!("{e}"))?;
 let direct=format!("candidate:1 1 UDP 2130706431 {} 9002 typ host",xbox_ip);
 rtc.add_remote_candidate(Candidate::from_sdp_string(&direct).map_err(|e|anyhow!("{e}"))?);
 for c in offer.lines().filter_map(|l|l.strip_prefix("a=")).filter(|l|l.starts_with("candidate:")){let _=client.send_ice_candidate(path,c).await;}
 println!("LAN_ONLY peer={} local={}",xbox_ip,local.ip());
 let mut relay=match std::env::var("OUTPUT_RTMP"){Ok(url)=>Some(MediaRelay::new(&url)?),Err(_)=>None};
 let mut last_video=Instant::now();let mut input_seq=0u32;let mut last_keyframe=Instant::now();
 
 let started=Instant::now();let mut vf=0u64;let mut af=0u64;let mut vb=0u64;let mut ab=0u64;let mut keyframes=0u64;
 let mut connected=false;let mut handshake=false;let mut buf=vec![0u8;65536];
 let mut tick=tokio::time::interval(Duration::from_millis(500));let mut last_print=Instant::now();
 loop{
  let deadline=loop{
   match rtc.poll_output().map_err(|e|anyhow!("{e}"))?{
    Output::Timeout(t)=>break t,
    Output::Transmit(t)=>{if t.destination.ip()==xbox_ip{let _=udp.send_to(&t.contents,t.destination).await;}}
    Output::Event(ev)=>match ev{
     Event::Connected=>{connected=true;println!("WEBRTC_CONNECTED seconds={:.2}",started.elapsed().as_secs_f64());}
     Event::IceConnectionStateChange(s)=>println!("ICE {:?}",s),
     Event::ChannelOpen(id,label)=>{println!("CHANNEL {}",label);if label=="message"{write_dc(&mut rtc,Some(id),&protocol::Handshake::new());}}
     Event::ChannelData(data)=>{
      match protocol::parse_message(&data.data){
      InboundMsg::HandshakeAck if !handshake=>{handshake=true;println!("XBOX_HANDSHAKE_ACK");write_dc(&mut rtc,control,&protocol::control_authorization());write_dc(&mut rtc,control,&protocol::gamepad_changed());
       let mut removed=protocol::gamepad_changed();removed.was_added=false;write_dc(&mut rtc,control,&removed);
       let bytes=xsx_native::rtc::input::encode_client_metadata(input_seq,started.elapsed().as_secs_f64()*1000.0);input_seq=input_seq.wrapping_add(1);if let Some(id)=input {if let Some(mut ch)=rtc.channel(id){let _=ch.write(true,&bytes);}}println!("CLIENT_METADATA_SENT_AFTER_AUTH");
       for m in protocol::config_messages(uuid){write_dc(&mut rtc,message,&m);}write_dc(&mut rtc,control,&protocol::keyframe_request());}
      InboundMsg::ServerDisconnect(reason)=>bail!("Server disconnected: {:?}",reason),_=>{}
     }},
     Event::MediaData(data)=>{
      if data.mid==video_mid{
       if vf==0{println!("VIDEO_CODEC {:?}",data.params.spec().codec);}
       vf+=1;vb+=data.data.len()as u64;if data.is_keyframe(){keyframes+=1;}
       last_video=Instant::now();
       if let Some(out)=relay.as_mut(){out.video(&data)?;}
      }else{if af==0{println!("AUDIO_CODEC {:?}",data.params.spec().codec);}af+=1;ab+=data.data.len()as u64;if let Some(out)=relay.as_mut(){out.audio(&data)?;}}
     },_=>{}
    }
   }
  };
  if last_print.elapsed()>Duration::from_secs(5){
   println!("MEDIA video_frames={} audio_packets={} video_bytes={} audio_bytes={} keyframes={}",vf,af,vb,ab,keyframes);
   xsx_native::health::update(if vf>0&&af>0{"streaming"}else{"connecting"},serde_json::json!({"control_revision":revision,"video_frames":vf,"audio_packets":af,"video_bytes":vb,"audio_bytes":ab,"keyframes":keyframes,"elapsed_seconds":started.elapsed().as_secs(),"lan_peer":xbox_ip.to_string()}))?;
   last_print=Instant::now();
  }
  if !rtc.is_alive(){bail!("WebRTC connection closed");}
  if !connected&&started.elapsed()>Duration::from_secs(25){bail!("LAN connection timed out");}
  if connected&&last_video.elapsed()>Duration::from_secs(20){bail!("Video receive watchdog timed out");}
  tokio::select!{
   r=udp.recv_from(&mut buf)=>{let(n,src)=r?;if src.ip()==xbox_ip{let input=Receive::new(Protocol::Udp,src,local,&buf[..n]).map_err(|e|anyhow!("{e}"))?;rtc.handle_input(Input::Receive(Instant::now(),input)).map_err(|e|anyhow!("{e}"))?;}}
   _=tokio::time::sleep(deadline.saturating_duration_since(Instant::now()))=>{rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e|anyhow!("{e}"))?;}
   _=tick.tick()=>{
    if let Some(out)=relay.as_mut(){out.tick()?;}
    if !connected{if let Ok(candidates)=client.poll_ice_candidates(path).await{for c in candidates{let parts=c.candidate.split_whitespace().collect::<Vec<_>>();if parts.get(4).is_some_and(|v|*v==xbox_ip.to_string()){if let Ok(c)=Candidate::from_sdp_string(&c.candidate){rtc.add_remote_candidate(c);}}}}}
    if handshake {
     if last_keyframe.elapsed()>Duration::from_secs(2){write_dc(&mut rtc,control,&protocol::keyframe_request());last_keyframe=Instant::now();}
     if let Some(id)=input{let mut bytes=[0u8;15];bytes[0..2].copy_from_slice(&2u16.to_le_bytes());bytes[2..6].copy_from_slice(&input_seq.to_le_bytes());bytes[6..14].copy_from_slice(&(started.elapsed().as_secs_f64()*1000.0).to_le_bytes());input_seq=input_seq.wrapping_add(1);if let Some(mut ch)=rtc.channel(id){let _=ch.write(true,&bytes);}}
    }
   }
  }
 }
}
