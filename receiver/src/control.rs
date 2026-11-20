use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlState {
    pub enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
}
impl Default for ControlState {
    fn default() -> Self {
        Self { enabled: true, revision: 0, updated_at: chrono::Utc::now().timestamp() }
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action { Start, Stop, Restart }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command { pub action: Action }

impl ControlState {
    pub fn apply(&self, action: Action) -> Self {
        let enabled = !matches!(action, Action::Stop);
        if enabled == self.enabled && !matches!(action, Action::Restart) { return self.clone(); }
        Self { enabled, revision: self.revision.saturating_add(1), updated_at: chrono::Utc::now().timestamp() }
    }
    fn path() -> PathBuf {
        PathBuf::from(std::env::var("STATE_DIR").unwrap_or_else(|_| "/state".into())).join("control.json")
    }
    pub fn load() -> Result<Self> {
        let p = Self::path();
        match std::fs::read(&p) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let initial = Self::default(); initial.save()?; Ok(initial)
            }
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self) -> Result<()> {
        let p = Self::path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension(format!("tmp-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        let result = (|| -> Result<()> {
            f.write_all(&serde_json::to_vec(self)?)?; f.sync_all()?;
            std::fs::rename(&tmp, &p)?; Ok(())
        })();
        if result.is_err() { let _ = std::fs::remove_file(&tmp); }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_is_persistent_and_start_is_idempotent() {
        let original = ControlState::default();
        assert_eq!(original.apply(Action::Start), original);
        let stopped = original.apply(Action::Stop);
        assert!(!stopped.enabled); assert_eq!(stopped.revision, 1);
        assert_eq!(stopped.apply(Action::Stop), stopped);
        let stored: ControlState = serde_json::from_slice(&serde_json::to_vec(&stopped).unwrap()).unwrap();
        assert!(!stored.enabled);
        let running = stored.apply(Action::Start);
        assert!(running.enabled); assert_eq!(running.revision, 2);
    }
    #[test]
    fn restart_always_changes_revision() {
        let original = ControlState::default();
        let again = original.apply(Action::Restart);
        assert!(again.enabled); assert_eq!(again.revision, original.revision + 1);
    }
    #[test]
    fn rejects_arbitrary_actions_and_extra_fields() {
        assert!(serde_json::from_str::<Command>(r#"{"action":"shell"}"#).is_err());
        assert!(serde_json::from_str::<Command>(r#"{"action":"start","command":"anything"}"#).is_err());
    }
}
