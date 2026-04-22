use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::domain::error::DomainError;
use crate::domain::session::{SessionId, WorktreeSession};

/// .worktrees/.sessions.json にセッション情報を永続化する。
///
/// 並行 save で lost update を起こさないよう、`.worktrees/.sessions.json.lock` を別ファイルとして
/// 用意し `fs2::FileExt::lock_exclusive()` で全 read-modify-write を排他化する。
/// lock file を JSON 本体と別にしているのは Windows 互換 (開いているファイルへの上書き制約回避)。
pub struct SessionStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl SessionStore {
    pub fn new(repo_root: &Path) -> Self {
        let dir = repo_root.join(".worktrees");
        Self {
            path: dir.join(".sessions.json"),
            lock_path: dir.join(".sessions.json.lock"),
        }
    }

    /// lock file を open (存在しなければ create) して排他ロックを取り、closure 実行後に unlock する。
    ///
    /// closure 内で panic した場合も `File` drop 時に OS が lock を解放するので fail-safe。
    fn with_lock<F, R>(&self, f: F) -> Result<R, DomainError>
    where
        F: FnOnce() -> Result<R, DomainError>,
    {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(|e| DomainError::Lock(format!("open lock file: {e}")))?;
        lock_file
            .lock_exclusive()
            .map_err(|e| DomainError::Lock(format!("acquire exclusive lock: {e}")))?;

        let result = f();

        // 明示 unlock。drop 時にも OS が外すが、早期解放して contention を減らす。
        let _ = FileExt::unlock(&lock_file);
        drop(lock_file);
        result
    }

    pub fn load(&self) -> Result<Vec<WorktreeSession>, DomainError> {
        self.with_lock(|| self.load_unlocked())
    }

    #[allow(dead_code)] // public API retained for external callers; internal code uses register/unregister.
    pub fn save(&self, sessions: &[WorktreeSession]) -> Result<(), DomainError> {
        self.with_lock(|| self.save_unlocked(sessions))
    }

    pub fn register(
        &self,
        worktree_name: &str,
        branch: &str,
        session_id: &SessionId,
    ) -> Result<(), DomainError> {
        self.with_lock(|| {
            let mut sessions = self.load_unlocked()?;
            sessions.push(WorktreeSession {
                worktree_name: worktree_name.to_string(),
                branch: branch.to_string(),
                session_id: session_id.clone(),
                created_at: chrono_now(),
            });
            self.save_unlocked(&sessions)
        })
    }

    pub fn unregister(&self, worktree_name: &str) -> Result<(), DomainError> {
        self.with_lock(|| {
            let mut sessions = self.load_unlocked()?;
            sessions.retain(|s| s.worktree_name != worktree_name);
            self.save_unlocked(&sessions)
        })
    }

    /// worktreeのオーナーセッションIDを返す。未登録ならNone。
    #[allow(dead_code)] // public API retained for external callers; verify_owner is used internally.
    pub fn owner_of(&self, worktree_name: &str) -> Result<Option<SessionId>, DomainError> {
        self.with_lock(|| {
            let sessions = self.load_unlocked()?;
            Ok(sessions
                .iter()
                .find(|s| s.worktree_name == worktree_name)
                .map(|s| s.session_id.clone()))
        })
    }

    /// セッションIDが一致するか検証。不一致ならSessionMismatchエラー。
    pub fn verify_owner(&self, worktree_name: &str, caller: &SessionId) -> Result<(), DomainError> {
        self.with_lock(|| {
            let sessions = self.load_unlocked()?;
            let found = sessions.iter().find(|s| s.worktree_name == worktree_name);
            match found {
                Some(s) if s.session_id == *caller => Ok(()),
                Some(s) => Err(DomainError::SessionMismatch {
                    worktree: worktree_name.to_string(),
                    owner: s.session_id.to_string(),
                    caller: caller.to_string(),
                }),
                None => Ok(()), // 未登録のworktreeは制限なし
            }
        })
    }

    /// Lock を取らずに JSON を読む。`with_lock` 経由でのみ呼ばれる。
    fn load_unlocked(&self) -> Result<Vec<WorktreeSession>, DomainError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read_to_string(&self.path)?;
        let sessions: Vec<WorktreeSession> =
            serde_json::from_str(&data).map_err(|e| DomainError::Git(e.to_string()))?;
        Ok(sessions)
    }

    /// Lock を取らずに JSON を書く。`with_lock` 経由でのみ呼ばれる。
    fn save_unlocked(&self, sessions: &[WorktreeSession]) -> Result<(), DomainError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data =
            serde_json::to_string_pretty(sessions).map_err(|e| DomainError::Git(e.to_string()))?;
        std::fs::write(&self.path, data)?;
        Ok(())
    }
}

// `File` の drop 時 lock 解放を OS に任せるため、ここでは何も実装しない。
// `fs2` は Drop 時の自動 unlock を保証しないプラットフォームがあるが、process 終了時
// OS が fd を閉じる時点で確実に解放される。
#[allow(dead_code)]
fn _lock_sanity(_f: &File) {}

fn chrono_now() -> String {
    // 外部crate不要。UNIXタイムスタンプで十分。
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn lock_serializes_sequential_register() {
        let tmp = TempDir::new().expect("tempdir");
        let store = SessionStore::new(tmp.path());
        let sid = SessionId::new();
        store
            .register("wt-a", "branch-a", &sid)
            .expect("register a");
        store
            .register("wt-b", "branch-b", &sid)
            .expect("register b");
        let all = store.load().expect("load");
        assert_eq!(all.len(), 2);
    }

    /// 10 task 並行 register で lost update が起きないこと。
    /// spawn_blocking で OS thread に分散させ、fs2 exclusive lock の直列化に依存する。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_save_no_lost_update() {
        let tmp = TempDir::new().expect("tempdir");
        let base = Arc::new(tmp.path().to_path_buf());
        let mut handles = Vec::new();
        for i in 0..10 {
            let p = Arc::clone(&base);
            handles.push(tokio::task::spawn_blocking(move || {
                let s = SessionStore::new(&p);
                let sid = SessionId::new();
                s.register(&format!("wt-{i}"), &format!("branch-{i}"), &sid)
                    .expect("register");
            }));
        }
        for h in handles {
            h.await.expect("join");
        }
        let all = SessionStore::new(&base).load().expect("final load");
        assert_eq!(all.len(), 10, "all 10 registrations must persist");
    }

    #[test]
    fn verify_owner_matches_caller() {
        let tmp = TempDir::new().expect("tempdir");
        let store = SessionStore::new(tmp.path());
        let sid = SessionId::new();
        store.register("wt-x", "b-x", &sid).expect("register");
        store.verify_owner("wt-x", &sid).expect("verify ok");
    }

    #[test]
    fn verify_owner_rejects_other_caller() {
        let tmp = TempDir::new().expect("tempdir");
        let store = SessionStore::new(tmp.path());
        let sid_a = SessionId::new();
        let sid_b = SessionId::new();
        store.register("wt-x", "b-x", &sid_a).expect("register");
        let err = store.verify_owner("wt-x", &sid_b).expect_err("mismatch");
        matches!(err, DomainError::SessionMismatch { .. });
    }
}
