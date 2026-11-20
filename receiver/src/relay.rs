use anyhow::{Result,bail};
use std::{path::PathBuf,process::{Child,ChildStdin,Command,Stdio},time::Instant,fs::OpenOptions,io::Write,collections::VecDeque,os::unix::fs::OpenOptionsExt};
use str0m::media::MediaData;
fn id(n:u32)->Vec<u8>{let a=n.to_be_bytes();a[a.iter().position(|v|*v!=0).unwrap_or(3)..].to_vec()}
fn size(n:usize)->Vec<u8>{for width in 1..=8{if (n as u64)<(1u64<<(7*width))-1{let v=(1u64<<(7*width))|n as u64;return v.to_be_bytes()[8-width..].to_vec();}}unreachable!()}
fn element(tag:u32,data:&[u8])->Vec<u8>{let mut b=id(tag);b.extend(size(data.len()));b.extend_from_slice(data);b}
fn uint(tag:u32,n:u64)->Vec<u8>{let a=n.to_be_bytes();element(tag,&a[a.iter().position(|v|*v!=0).unwrap_or(7)..])}
fn text(tag:u32,s:&str)->Vec<u8>{element(tag,s.as_bytes())}
fn join(v:&[Vec<u8>])->Vec<u8>{v.concat()}
fn nals(b:&[u8])->Vec<&[u8]>{
 let mut starts=vec![];let mut i=0;while i+3<=b.len(){if i+4<=b.len()&&b[i..i+4]==[0,0,0,1]{starts.push((i,i+4));i+=4;}else if b[i..i+3]==[0,0,1]{starts.push((i,i+3));i+=3;}else{i+=1;}}
 if starts.is_empty(){return if b.is_empty(){vec![]}else{vec![b]};}
 starts.iter().enumerate().filter_map(|(j,(_,from))|{let to=starts.get(j+1).map(|v|v.0).unwrap_or(b.len());if *from<to{Some(&b[*from..to])}else{None}}).collect()
}
struct Bits{b:Vec<u8>,i:usize}
impl Bits{
 fn bit(&mut self)->Result<u32>{if self.i>=self.b.len()*8{bail!("Truncated H264 SPS");}let v=(self.b[self.i/8]>>(7-self.i%8))&1;self.i+=1;Ok(v as u32)}
 fn n(&mut self,n:usize)->Result<u32>{let mut v=0;for _ in 0..n{v=(v<<1)|self.bit()?;}Ok(v)}
 fn ue(&mut self)->Result<u32>{let mut zeros=0;while self.bit()?==0{zeros+=1;if zeros>30{bail!("Invalid SPS integer");}}Ok(((1u32<<zeros)-1)+self.n(zeros)?)}
 fn se(&mut self)->Result<i32>{let n=self.ue()?;Ok(if n%2==1{((n+1)/2)as i32}else{-((n/2)as i32)})}
}
fn dimensions(s:&[u8])->Result<(u32,u32)>{
 let mut b=Vec::new();let mut zero=0;for &v in &s[1..]{if zero>=2&&v==3{zero=0;continue;}b.push(v);zero=if v==0{zero+1}else{0};}
 let mut r=Bits{b,i:0};let profile=r.n(8)?;r.n(16)?;r.ue()?;let mut chroma=1;
 if [100,110,122,244,44,83,86,118,128,138,139,134,135].contains(&profile){
  chroma=r.ue()?;if chroma==3&&r.bit()?==1{chroma=0;}r.ue()?;r.ue()?;r.bit()?;
  if r.bit()?==1{for index in 0..if chroma==3{12}else{8}{if r.bit()?==1{let mut last=8i32;let mut next=8i32;for _ in 0..if index<6{16}else{64}{if next!=0{next=(last+r.se()?+256)%256;}if next!=0{last=next;}}}}}
 }
 r.ue()?;let poc=r.ue()?;if poc==0{r.ue()?;}else if poc==1{r.bit()?;r.se()?;r.se()?;let count=r.ue()?;if count>255{bail!("Invalid SPS cycle");}for _ in 0..count{r.se()?;}}
 r.ue()?;r.bit()?;let w=(r.ue()?+1)*16;let hm=r.ue()?+1;let frame=r.bit()?;if frame==0{r.bit()?;}r.bit()?;let(mut left,mut right,mut top,mut bottom)=(0,0,0,0);
 if r.bit()?==1{left=r.ue()?;right=r.ue()?;top=r.ue()?;bottom=r.ue()?;}
 let cx=if chroma==1||chroma==2{2}else{1};let cy=if chroma==1{2*(2-frame)}else{2-frame};
 let width=w.saturating_sub((left+right)*cx);let height=((2-frame)*hm*16).saturating_sub((top+bottom)*cy);
 if width==0||height==0||width>8192||height>8192{bail!("Invalid source dimensions");}Ok((width,height))
}
struct Clock{first:Option<(u64,u64)>}
impl Clock{fn timestamp(&mut self,data:&MediaData,elapsed:u64)->u64{let now=data.time.as_micros();let(src,out)=*self.first.get_or_insert((now,elapsed));out.saturating_add(now.saturating_sub(src)) /1000}}
pub struct MediaRelay{
 child:Child,pipe:ChildStdin,started:Instant,video_clock:Clock,audio_clock:Clock,sps:Vec<u8>,pps:Vec<u8>,ready:bool,await_key:bool,cluster:Option<u64>,queued_audio:VecDeque<(u64,Vec<u8>)>,pub video_frames:u64,pub audio_packets:u64
}
impl MediaRelay{
 pub fn new(url:&str)->Result<Self>{
  let dir=PathBuf::from(std::env::var("STATE_DIR").unwrap_or_else(|_|"/state".into()));std::fs::create_dir_all(&dir)?;
  let log=dir.join("ffmpeg.log");if log.metadata().map(|m|m.len()>2*1024*1024).unwrap_or(false){let _=std::fs::rename(&log,dir.join("ffmpeg.previous.log"));}
  let err=OpenOptions::new().create(true).append(true).mode(0o600).open(log)?;
  let mut child=Command::new("/usr/bin/ffmpeg").args(["-hide_banner","-nostdin","-loglevel","warning","-fflags","+genpts+discardcorrupt","-analyzeduration","1000000","-probesize","2000000","-f","matroska","-i","pipe:0","-map","0:v:0","-map","0:a:0","-c:v","copy","-c:a","aac","-ar","48000","-ac","2","-b:a","160k","-af","aresample=async=1:first_pts=0","-max_interleave_delta","1000000","-flvflags","no_duration_filesize","-f","flv","-rw_timeout","8000000"]).arg(url).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(err).spawn()?;
  let pipe=child.stdin.take().expect("piped FFmpeg input");
  Ok(Self{child,pipe,started:Instant::now(),video_clock:Clock{first:None},audio_clock:Clock{first:None},sps:vec![],pps:vec![],ready:false,await_key:true,cluster:None,queued_audio:VecDeque::new(),video_frames:0,audio_packets:0})
 }
 fn header(&mut self)->Result<()>{
  let(w,h)=dimensions(&self.sps)?;
  let ebml=element(0x1A45DFA3,&join(&[uint(0x4286,1),uint(0x42F7,1),uint(0x42F2,4),uint(0x42F3,8),text(0x4282,"matroska"),uint(0x4287,4),uint(0x4285,2)]));
  self.pipe.write_all(&ebml)?;self.pipe.write_all(&id(0x18538067))?;self.pipe.write_all(&[1,255,255,255,255,255,255,255])?;
  self.pipe.write_all(&element(0x1549A966,&join(&[uint(0x2AD7B1,1_000_000),text(0x4D80,"xsx-native"),text(0x5741,"xsx-native") ])))?;
  if self.sps.len()<4{bail!("H264 configuration too short");}
  let mut avcc=vec![1,self.sps[1],self.sps[2],self.sps[3],255,225];avcc.extend_from_slice(&(self.sps.len()as u16).to_be_bytes());avcc.extend_from_slice(&self.sps);avcc.push(1);avcc.extend_from_slice(&(self.pps.len()as u16).to_be_bytes());avcc.extend_from_slice(&self.pps);
  let video=element(0xAE,&join(&[uint(0xD7,1),uint(0x73C5,1),uint(0x83,1),uint(0x9C,0),text(0x86,"V_MPEG4/ISO/AVC"),element(0x63A2,&avcc),uint(0x23E383,16_683_333),element(0xE0,&join(&[uint(0xB0,w as u64),uint(0xBA,h as u64)]))]));
  let mut opus=b"OpusHead".to_vec();opus.extend_from_slice(&[1,2]);opus.extend_from_slice(&312u16.to_le_bytes());opus.extend_from_slice(&48000u32.to_le_bytes());opus.extend_from_slice(&[0,0,0]);
  let audio=element(0xAE,&join(&[uint(0xD7,2),uint(0x73C5,2),uint(0x83,2),uint(0x9C,0),text(0x86,"A_OPUS"),element(0x63A2,&opus),uint(0x56AA,6_500_000),uint(0x56BB,80_000_000),element(0xE1,&join(&[element(0xB5,&48000f64.to_be_bytes()),uint(0x9F,2)]))]));
  self.pipe.write_all(&element(0x1654AE6B,&join(&[video,audio])))?;self.ready=true;println!("MUX_READY width={w} height={h} video_copy=true");Ok(())
 }
 fn block(&mut self,track:u8,pts:u64,key:bool,data:&[u8])->Result<()>{
  if self.cluster.is_none_or(|t|pts.saturating_sub(t)>=1000||pts<t.saturating_sub(30000)){let base=pts/1000*1000;self.pipe.write_all(&id(0x1F43B675))?;self.pipe.write_all(&[1,255,255,255,255,255,255,255])?;self.pipe.write_all(&uint(0xE7,base))?;self.cluster=Some(base);}
  let rel=pts as i64-self.cluster.unwrap()as i64;if !(-32768..=32767).contains(&rel){bail!("Media interleave out of range");}
  let mut b=Vec::with_capacity(4+data.len());b.push(0x80|track);b.extend_from_slice(&(rel as i16).to_be_bytes());b.push(if key{0x80}else{0});b.extend_from_slice(data);self.pipe.write_all(&element(0xA3,&b))?;Ok(())
 }
 pub fn video(&mut self,data:&MediaData)->Result<()>{
  let pts=self.video_clock.timestamp(data,self.started.elapsed().as_micros()as u64);let units=nals(&data.data);let key=data.is_keyframe()||units.iter().any(|n|n[0]&31==5);
  for n in &units{match n[0]&31{7=>{if self.ready&&self.sps!=*n{bail!("Source video configuration changed; reconnecting");}self.sps=n.to_vec();},8=>self.pps=n.to_vec(),_=>{}}}
  if !data.contiguous&&!key{self.await_key=true;}
  if self.await_key&&!key{return Ok(());}if self.sps.is_empty()||self.pps.is_empty(){return Ok(());}
  if !self.ready{self.header()?;while let Some((t,b))=self.queued_audio.pop_front(){self.block(2,t,true,&b)?;}}
  self.await_key=false;let mut frame=Vec::with_capacity(data.data.len());for nal in units{frame.extend_from_slice(&(nal.len()as u32).to_be_bytes());frame.extend_from_slice(nal);}
  self.block(1,pts,key,&frame)?;self.video_frames+=1;Ok(())
 }
 pub fn audio(&mut self,data:&MediaData)->Result<()>{
  let pts=self.audio_clock.timestamp(data,self.started.elapsed().as_micros()as u64);if self.ready{self.block(2,pts,true,&data.data)?;}else{if self.queued_audio.len()>=200{self.queued_audio.pop_front();}self.queued_audio.push_back((pts,data.data.to_vec()));}
  self.audio_packets+=1;Ok(())
 }
 pub fn tick(&mut self)->Result<()>{if let Some(status)=self.child.try_wait()?{bail!("FFmpeg relay exited: {status}");}Ok(())}
}
impl Drop for MediaRelay{fn drop(&mut self){let _=self.pipe.flush();let _=self.child.kill();let _=self.child.wait();}}
