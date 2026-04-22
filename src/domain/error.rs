use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("session mismatch: worktree '{worktree}' belongs to session '{owner}', not '{caller}'")]
    SessionMismatch {
        worktree: String,
        owner: String,
        caller: String,
    },

    #[error("worktree not found: {0}")]
    WorktreeNotFound(String),

    #[error("worktree already exists: {0}")]
    WorktreeAlreadyExists(String),

    #[error("branch not found: {0}")]
    BranchNotFound(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file lock error: {0}")]
    Lock(String),
}
