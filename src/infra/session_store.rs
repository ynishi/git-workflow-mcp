use std::path::{Path, PathBuf};

use crate::domain::error::DomainError;
use crate::domain::session::{SessionId, WorktreeSession};

/// .worktrees/.sessions.json にセッション情報を永続化する。
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            path: repo_root.join(".worktrees").join(".sessions.json"),
        }
    }

    pub fn load(&self) -> Result<Vec<WorktreeSession>, DomainError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read_to_string(&self.path)?;
        let sessions: Vec<WorktreeSession> =
            serde_json::from_str(&data).map_err(|e| DomainError::Git(e.to_string()))?;
        Ok(sessions)
    }

    pub fn save(&self, sessions: &[WorktreeSession]) -> Result<(), DomainError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data =
            serde_json::to_string_pretty(sessions).map_err(|e| DomainError::Git(e.to_string()))?;
        std::fs::write(&self.path, data)?;
        Ok(())
    }

    pub fn register(
        &self,
        worktree_name: &str,
        branch: &str,
        session_id: &SessionId,
    ) -> Result<(), DomainError> {
        let mut sessions = self.load()?;
        sessions.push(WorktreeSession {
            worktree_name: worktree_name.to_string(),
            branch: branch.to_string(),
            session_id: session_id.clone(),
            created_at: chrono_now(),
        });
        self.save(&sessions)
    }

    pub fn unregister(&self, worktree_name: &str) -> Result<(), DomainError> {
        let mut sessions = self.load()?;
        sessions.retain(|s| s.worktree_name != worktree_name);
        self.save(&sessions)
    }

    /// worktreeのオーナーセッションIDを返す。未登録ならNone。
    pub fn owner_of(&self, worktree_name: &str) -> Result<Option<SessionId>, DomainError> {
        let sessions = self.load()?;
        Ok(sessions
            .iter()
            .find(|s| s.worktree_name == worktree_name)
            .map(|s| s.session_id.clone()))
    }

    /// セッションIDが一致するか検証。不一致ならSessionMismatchエラー。
    pub fn verify_owner(&self, worktree_name: &str, caller: &SessionId) -> Result<(), DomainError> {
        match self.owner_of(worktree_name)? {
            Some(owner) if owner == *caller => Ok(()),
            Some(owner) => Err(DomainError::SessionMismatch {
                worktree: worktree_name.to_string(),
                owner: owner.to_string(),
                caller: caller.to_string(),
            }),
            None => Ok(()), // 未登録のworktreeは制限なし
        }
    }
}

fn chrono_now() -> String {
    // 外部crate不要。UNIXタイムスタンプで十分。
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}
