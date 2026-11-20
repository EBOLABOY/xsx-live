use anyhow::{Result, bail};
use serde_json::{json,Value};
use std::path::PathBuf;
fn path()->PathBuf {PathBuf::from(std::env::var("STATE_DIR").unwrap_or_else(|_|"/state".into())).join("status.json")}
pub fn update(state:&str,details:Value)->Result<()> {
 let p=path();std::fs::create_dir_all(p.parent().unwrap())?;
 let value=json!({"state":state,"updated_at":chrono::Utc::now().timestamp(),"details":details});
 let tmp=p.with_extension("tmp");std::fs::write(&tmp,serde_json::to_vec(&value)?)?;std::fs::rename(tmp,p)?;Ok(())
}
pub fn check()->Result<()> {
 let value:Value=serde_json::from_slice(&std::fs::read(path())?)?;
 let age=chrono::Utc::now().timestamp()-value["updated_at"].as_i64().unwrap_or(0);
 let state=value["state"].as_str().unwrap_or("unknown");
 let limit=if state=="connecting"{180}else if state=="waiting_console"{90}else{35};
 if age<0||age>limit||state=="stopped" {bail!("Receiver status is stale or stopped");} Ok(())
}
