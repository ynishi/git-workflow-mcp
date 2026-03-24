/// git worktreeの情報。`git worktree list --porcelain` から構築。
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub is_bare: bool,
}

/// git statusの要約。
#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: String,
    pub clean: bool,
    pub changed_files: Vec<String>,
}

/// git diffの結果。
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub stat: String,
    pub diff: String,
}

/// git logのエントリ。
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub hash: String,
    pub message: String,
}

/// commitの結果。
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub hash: String,
    pub message: String,
    pub files_changed: usize,
}

/// mergeの結果。
#[derive(Debug, Clone)]
pub struct MergeResult {
    pub merged_branch: String,
    pub into_branch: String,
    pub summary: String,
}

/// safe resetの結果。
#[derive(Debug, Clone)]
pub struct ResetResult {
    /// reset前のHEADのshort hash
    pub previous_head: String,
    /// reset後のHEADのshort hash
    pub new_head: String,
    /// "soft" or "mixed"
    pub mode: String,
}
