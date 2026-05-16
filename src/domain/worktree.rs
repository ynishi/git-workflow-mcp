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
#[derive(Debug, Clone, serde::Serialize)]
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

/// `git remote -v` の1エントリ。
// Used by infra::git::remote_list; wired to interface layer in Subtask 2.
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteEntry {
    pub name: String,
    pub url: String,
    /// "fetch" または "push"
    pub direction: String,
}

/// Branch と base の ahead/behind 状態。
///
/// `ahead` / `behind` は typed な `u32` count。文字列整形 field は持たない。
/// `git rev-list --left-right --count <base>...<branch>` (3-dots) で取得し、
/// output split[0]=behind, split[1]=ahead の順で parse する。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BranchStatus {
    /// base より branch が ahead な commit 数
    pub ahead: u32,
    /// branch より base が ahead な commit 数 (branch が behind)
    pub behind: u32,
    /// ahead == 0 && behind == 0
    pub up_to_date: bool,
    /// branch にあり base にない commit 一覧 (ahead commits)
    pub ahead_commits: Vec<LogEntry>,
    /// base にあり branch にない commit 一覧 (behind commits)
    pub behind_commits: Vec<LogEntry>,
    /// branch と base の共通 ancestor hash
    pub common_ancestor: String,
}

/// ローカル branch にあり remote tracking ref にない commit の一覧。
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnpushedCommits {
    pub commits: Vec<LogEntry>,
    pub count: u32,
    /// remote tracking ref の HEAD hash
    pub remote_head: String,
}

/// commit が remote tracking ref から reachable かどうか。
#[derive(Debug, Clone, serde::Serialize)]
pub struct IsPushedResult {
    /// commit が 1 つ以上の remote tracking ref に含まれる場合 true
    pub pushed: bool,
    /// commit を含む remote ref 名の一覧 (例: "refs/remotes/origin/main")
    pub refs: Vec<String>,
}
