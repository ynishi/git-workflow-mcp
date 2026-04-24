use std::path::Path;
use std::process::Command;

use crate::domain::error::DomainError;
use crate::domain::worktree::{
    CommitResult, DiffResult, LogEntry, MergeResult, RemoteEntry, RepoStatus, ResetResult, Worktree,
};

fn run_git(repo: &Path, args: &[&str]) -> Result<String, DomainError> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(args)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(DomainError::Git(stderr))
    }
}

// ─── Worktree ────────────────────────────────────────────

/// Checks whether a local branch `branch` exists in `repo`.
///
/// Uses `git show-ref --verify --quiet refs/heads/<branch>` for exact-match
/// semantics, avoiding glob interpretation of `git branch --list <pattern>`.
fn branch_exists(repo: &Path, branch: &str) -> Result<bool, DomainError> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()?;
    Ok(output.status.success())
}

/// Creates a new git worktree under `{repo}/.worktrees/{worktree_name}` on a new `branch`.
///
/// # Parameters
///
/// - `base_branch`: when `Some(base)`, the new `branch` is created from `base`
///   (equivalent to `git branch <new> <base>`). When `None`, the branch is
///   created from the current `HEAD` of `repo`.
///
/// # Preconditions (fail-fast)
///
/// Applied regardless of `base_branch` being set:
///
/// 1. `{repo}/.worktrees/{worktree_name}` does not exist.
/// 2. `branch` does not yet exist as a local branch (exact-match via `show-ref`).
///
/// Applied only when `base_branch` is `Some`:
///
/// 3. If `base_branch` is currently checked out in some worktree (main repo or
///    any linked worktree), that worktree must be clean (`git status --porcelain`
///    empty). This is a **best-effort** check:
///    - If `base_branch` is not checked out anywhere, the check is skipped
///      (a non-checked-out ref cannot be dirty by definition).
///    - There is an inherent TOCTOU window between the check and the subsequent
///      `git branch` command; callers must not rely on this as a strong invariant.
///    - The check can be bypassed entirely by setting environment variable
///      `GIT_WORKFLOW_ALLOW_DIRTY_BASE=1` (or `true`/`yes`).
///
/// # Returns
///
/// The absolute path of the created worktree as a `String`.
pub fn worktree_add(
    repo: &Path,
    worktree_name: &str,
    branch: &str,
    base_branch: Option<&str>,
) -> Result<String, DomainError> {
    // Read ENV on every call (not cached). Values "1"/"true"/"yes" enable skip.
    let allow_dirty = {
        let val = std::env::var("GIT_WORKFLOW_ALLOW_DIRTY_BASE").unwrap_or_default();
        matches!(val.as_str(), "1" | "true" | "yes")
    };
    worktree_add_impl(repo, worktree_name, branch, base_branch, allow_dirty)
}

fn worktree_add_impl(
    repo: &Path,
    worktree_name: &str,
    branch: &str,
    base_branch: Option<&str>,
    allow_dirty: bool,
) -> Result<String, DomainError> {
    let worktree_path = repo.join(".worktrees").join(worktree_name);
    if worktree_path.exists() {
        return Err(DomainError::WorktreeAlreadyExists(
            worktree_name.to_string(),
        ));
    }

    // Precondition: branch collision check (applies to both Some/None base_branch).
    if branch_exists(repo, branch)? {
        return Err(DomainError::Git(format!("branch already exists: {branch}")));
    }

    if let Some(base) = base_branch {
        // Precondition: dirty base_branch check (unless override).
        if !allow_dirty {
            let worktrees = worktree_list(repo)?;
            let base_worktree = worktrees
                .iter()
                .find(|wt| wt.branch.as_deref() == Some(base));
            if let Some(wt) = base_worktree {
                let wt_path = std::path::Path::new(&wt.path);
                let dirty = run_git(wt_path, &["status", "--porcelain"])?;
                if !dirty.trim().is_empty() {
                    return Err(DomainError::Git(format!(
                        "base_branch has uncommitted changes: {base} at {}",
                        wt.path
                    )));
                }
            }
        } else {
            tracing::warn!(
                base_branch = %base,
                "GIT_WORKFLOW_ALLOW_DIRTY_BASE is set; skipping dirty check for base_branch"
            );
        }

        // ブランチ作成（base から分岐）
        run_git(repo, &["branch", branch, base])?;
    } else {
        // base_branch なし: HEAD から分岐
        run_git(repo, &["branch", branch])?;
    }

    // worktree 作成
    run_git(
        repo,
        &["worktree", "add", &worktree_path.to_string_lossy(), branch],
    )?;

    Ok(worktree_path.to_string_lossy().to_string())
}

pub fn worktree_remove(repo: &Path, worktree_name: &str) -> Result<(), DomainError> {
    let worktree_path = repo.join(".worktrees").join(worktree_name);
    if !worktree_path.exists() {
        return Err(DomainError::WorktreeNotFound(worktree_name.to_string()));
    }
    run_git(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            &worktree_path.to_string_lossy(),
        ],
    )?;
    Ok(())
}

pub fn worktree_list(repo: &Path) -> Result<Vec<Worktree>, DomainError> {
    let output = run_git(repo, &["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(prev_path) = current_path.take() {
                worktrees.push(Worktree {
                    path: prev_path,
                    branch: current_branch.take(),
                    is_bare,
                });
                is_bare = false;
            }
            current_path = Some(path.to_string());
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = Some(branch_ref.trim_start_matches("refs/heads/").to_string());
        } else if line == "bare" {
            is_bare = true;
        }
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
            is_bare,
        });
    }

    Ok(worktrees)
}

// ─── Branch ──────────────────────────────────────────────

/// Deletes a local branch with `git branch -d` (safety check preserved).
///
/// # Parameters
///
/// - `working_dir`: when `Some(path)`, `git branch -d` is executed with `path`
///   as the working directory (typically a linked worktree). This is required
///   when the branch has been merged only into a non-default HEAD (e.g. a topic
///   worktree), because `git branch -d`'s merge check is evaluated against the
///   current HEAD of the running directory. When `None`, the command runs in
///   `repo` (repo root) for backward compatibility.
///
/// # Preconditions
///
/// If `working_dir` is `Some`:
/// 1. The path must exist and be a directory.
/// 2. Its canonicalized path must match either `repo` itself or one of the
///    worktrees registered in `repo` (via `git worktree list`). Arbitrary
///    directories — including unrelated git repositories — are rejected to
///    prevent accidentally deleting a same-named branch in a foreign repo.
pub fn branch_delete(
    repo: &Path,
    branch: &str,
    working_dir: Option<&Path>,
) -> Result<(), DomainError> {
    if let Some(wd) = working_dir {
        if !wd.is_dir() {
            return Err(DomainError::Git(format!(
                "working_dir does not exist or is not a directory: {}",
                wd.display()
            )));
        }
        let wd_canon = wd.canonicalize().map_err(|e| {
            DomainError::Git(format!(
                "failed to canonicalize working_dir {}: {e}",
                wd.display()
            ))
        })?;
        let repo_canon = repo.canonicalize().map_err(|e| {
            DomainError::Git(format!(
                "failed to canonicalize repo {}: {e}",
                repo.display()
            ))
        })?;
        let is_known = wd_canon == repo_canon || {
            let worktrees = worktree_list(repo)?;
            worktrees.iter().any(|w| {
                Path::new(&w.path)
                    .canonicalize()
                    .map(|p| p == wd_canon)
                    .unwrap_or(false)
            })
        };
        if !is_known {
            return Err(DomainError::Git(format!(
                "working_dir is neither repo root nor a known worktree of {}: {}",
                repo.display(),
                wd.display()
            )));
        }
    }
    let run_dir = working_dir.unwrap_or(repo);

    run_git(run_dir, &["branch", "-d", branch]).map_err(|e| match e {
        DomainError::Git(msg) if msg.contains("not found") => {
            DomainError::BranchNotFound(branch.to_string())
        }
        other => other,
    })?;
    Ok(())
}

// ─── Status / Diff ───────────────────────────────────────

#[derive(Default)]
pub struct DiffOptions {
    pub staged: bool,
    pub commit_range: Option<String>,
    pub paths: Option<Vec<String>>,
    pub name_only: bool,
}

pub fn status(working_dir: &Path) -> Result<RepoStatus, DomainError> {
    let branch = run_git(working_dir, &["branch", "--show-current"])?;
    let status_output = run_git(working_dir, &["status", "--short"])?;
    let changed_files: Vec<String> = status_output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    let clean = changed_files.is_empty();

    Ok(RepoStatus {
        branch,
        clean,
        changed_files,
    })
}

pub fn diff(working_dir: &Path, opts: &DiffOptions) -> Result<DiffResult, DomainError> {
    // name_only モード: stat は不要、name-only diff のみ返す
    if opts.name_only {
        let mut args = vec!["diff", "--name-only"];
        let range_owned;
        if let Some(ref range) = opts.commit_range {
            range_owned = range.clone();
            args.push(&range_owned);
        } else if opts.staged {
            args.push("--cached");
        }
        let paths_owned: Vec<String>;
        if let Some(ref paths) = opts.paths {
            args.push("--");
            paths_owned = paths.clone();
            for p in &paths_owned {
                args.push(p.as_str());
            }
        }
        let output = run_git(working_dir, &args)?;
        return Ok(DiffResult {
            stat: String::new(),
            diff: output,
        });
    }

    // stat + patch モード
    let range_owned;
    let paths_owned: Vec<String>;

    let mut stat_args = vec!["diff", "--stat"];
    let mut diff_args = vec!["diff"];

    if let Some(ref range) = opts.commit_range {
        range_owned = range.clone();
        stat_args.push(&range_owned);
        diff_args.push(&range_owned);
    } else if opts.staged {
        stat_args.push("--cached");
        diff_args.push("--cached");
    }

    if let Some(ref paths) = opts.paths {
        stat_args.push("--");
        diff_args.push("--");
        paths_owned = paths.clone();
        for p in &paths_owned {
            stat_args.push(p.as_str());
            diff_args.push(p.as_str());
        }
    }

    let stat = run_git(working_dir, &stat_args)?;
    let diff_output = run_git(working_dir, &diff_args)?;

    Ok(DiffResult {
        stat,
        diff: diff_output,
    })
}

// ─── Log ────────────────────────────────────────────────

pub fn log(
    working_dir: &Path,
    range: Option<&str>,
    max_count: u32,
) -> Result<Vec<LogEntry>, DomainError> {
    let mut args = vec!["log", "--oneline"];
    let max_count_str = format!("-{max_count}");
    args.push(&max_count_str);

    let range_owned;
    if let Some(r) = range {
        range_owned = r.to_string();
        args.push(&range_owned);
    }

    let output = run_git(working_dir, &args)?;
    let entries = output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (hash, message) = line.split_once(' ').unwrap_or((line, ""));
            LogEntry {
                hash: hash.to_string(),
                message: message.to_string(),
            }
        })
        .collect();

    Ok(entries)
}

// ─── Commit ──────────────────────────────────────────────

pub fn commit(working_dir: &Path, message: &str) -> Result<CommitResult, DomainError> {
    run_git(working_dir, &["add", "-A"])?;

    // 変更があるか確認
    let staged = run_git(working_dir, &["diff", "--cached", "--stat"])?;
    if staged.is_empty() {
        return Err(DomainError::Git("nothing to commit".to_string()));
    }

    run_git(working_dir, &["commit", "-m", message])?;

    let hash = run_git(working_dir, &["rev-parse", "--short", "HEAD"])?;
    let files_changed = staged.lines().count().saturating_sub(1); // last line is summary

    Ok(CommitResult {
        hash,
        message: message.to_string(),
        files_changed,
    })
}

// ─── Reset ───────────────────────────────────────────────

pub fn reset(working_dir: &Path, mode: &str, target: &str) -> Result<ResetResult, DomainError> {
    if mode != "soft" && mode != "mixed" {
        return Err(DomainError::Git(format!(
            "unsupported reset mode: {mode}. Only 'soft' and 'mixed' are allowed"
        )));
    }

    let previous_head = run_git(working_dir, &["rev-parse", "--short", "HEAD"])?;

    let mode_flag = format!("--{mode}");
    run_git(working_dir, &["reset", &mode_flag, target])?;

    let new_head = run_git(working_dir, &["rev-parse", "--short", "HEAD"])?;

    Ok(ResetResult {
        previous_head,
        new_head,
        mode: mode.to_string(),
    })
}

// ─── Merge ───────────────────────────────────────────────

/// Merges `branch` into `into_branch` using `git merge --no-ff`.
///
/// # Parameters
///
/// - `working_dir`: when `Some(path)`, the HEAD-match precondition and the
///   `git merge` command are executed with `path` as the working directory
///   (typically a linked worktree). When `None`, they run in `repo` (repo root).
///
/// # Preconditions
///
/// 1. If `working_dir` is `Some`, the path must exist and be a directory.
/// 2. The current branch of the resolved working directory must equal
///    `into_branch` (the C1 HEAD-match safety check).
pub fn merge(
    repo: &Path,
    branch: &str,
    into_branch: &str,
    working_dir: Option<&Path>,
) -> Result<MergeResult, DomainError> {
    if let Some(wd) = working_dir
        && !wd.is_dir()
    {
        return Err(DomainError::Git(format!(
            "working_dir does not exist or is not a directory: {}",
            wd.display()
        )));
    }
    let run_dir = working_dir.unwrap_or(repo);

    let current = run_git(run_dir, &["branch", "--show-current"])?;
    if current != into_branch {
        return Err(DomainError::Git(format!(
            "must be on branch '{into_branch}' to merge, currently on '{current}'"
        )));
    }

    let output = run_git(
        run_dir,
        &[
            "merge",
            branch,
            "--no-ff",
            "-m",
            &format!("merge: {branch}"),
        ],
    )?;

    Ok(MergeResult {
        merged_branch: branch.to_string(),
        into_branch: into_branch.to_string(),
        summary: output,
    })
}

// ─── Remote / Fetch ──────────────────────────────────────

/// Validates that `s` is a safe remote name (allowlist).
///
/// Allowed characters: ASCII alphanumeric, `/`, `.`, `_`, `-`.
/// Empty strings are rejected.
// Called by fetch(); also used directly from interface layer in Subtask 2.
pub fn validate_remote_name(s: &str) -> Result<(), DomainError> {
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        return Err(DomainError::Git(format!("invalid remote name: {s}")));
    }
    Ok(())
}

/// Validates that `s` is a safe git refspec (allowlist).
///
/// Allowed characters: ASCII alphanumeric, `/`, `.`, `_`, `-`, `*`, `+`, `~`, `^`, `:`.
/// Empty strings are rejected.
// Called by fetch(); also used directly from interface layer in Subtask 2.
pub fn validate_refspec(s: &str) -> Result<(), DomainError> {
    if s.is_empty()
        || !s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '/' | '.' | '_' | '-' | '*' | '+' | '~' | '^' | ':')
        })
    {
        return Err(DomainError::Git(format!("invalid refspec: {s}")));
    }
    Ok(())
}

/// Runs `git fetch` against `repo`.
///
/// - `remote`: remote name (default: `"origin"`). Validated via `validate_remote_name`.
/// - `refspec`: optional refspec. Validated via `validate_refspec`.
/// - `prune`: when `true`, adds `--prune` to delete stale tracking refs.
///
/// Returns the trimmed stdout of the fetch command.
// Wired to interface layer (tools.rs) in Subtask 2.
pub fn fetch(
    repo: &Path,
    remote: Option<&str>,
    refspec: Option<&str>,
    prune: bool,
) -> Result<String, DomainError> {
    if let Some(r) = remote {
        validate_remote_name(r)?;
    }
    if let Some(rs) = refspec {
        validate_refspec(rs)?;
    }

    // Build args. `run_git` takes `&[&str]`, so String values must be bound to
    // stack variables before taking a `&str` slice — same pattern as `diff`'s
    // `range_owned`.
    let remote_owned = remote.unwrap_or("origin").to_string();
    let refspec_owned: Option<String> = refspec.map(|s| s.to_string());

    let mut args: Vec<&str> = vec!["fetch"];
    if prune {
        args.push("--prune");
    }
    args.push(remote_owned.as_str());
    if let Some(ref rs) = refspec_owned {
        args.push(rs.as_str());
    }

    run_git(repo, &args)
}

/// Lists remotes via `git remote -v`.
///
/// Each output line has the format `<name>\t<url> (fetch|push)`.
/// Lines that cannot be parsed are silently skipped; an empty repository
/// returns an empty `Vec`.
// Wired to interface layer (tools.rs) in Subtask 2.
pub fn remote_list(repo: &Path) -> Result<Vec<RemoteEntry>, DomainError> {
    let output = run_git(repo, &["remote", "-v"])?;
    let mut entries = Vec::new();
    for line in output.lines() {
        // Expected: "origin\thttps://example.com/repo.git (fetch)"
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        // rest: "https://example.com/repo.git (fetch)"
        let Some((url_part, direction_part)) = rest.rsplit_once(' ') else {
            continue;
        };
        // direction_part: "(fetch)" or "(push)"
        let direction = direction_part.trim_start_matches('(').trim_end_matches(')');
        if direction.is_empty() {
            continue;
        }
        entries.push(RemoteEntry {
            name: name.to_string(),
            url: url_part.to_string(),
            direction: direction.to_string(),
        });
    }
    Ok(entries)
}

// ─── Unit tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    /// テスト用の git リポジトリを初期化し、initial commit を作成する。
    fn init_repo() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        StdCommand::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .expect("git init");
        StdCommand::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .expect("git config email");
        StdCommand::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .expect("git config name");
        std::fs::write(path.join("README.md"), "# test").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .expect("git add");
        StdCommand::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(path)
            .output()
            .expect("git commit");
        dir
    }

    /// ディレクトリ内でファイルを追加して commit する。
    fn add_commit(dir: &Path, filename: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(filename), content).unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .expect("git add");
        StdCommand::new("git")
            .args(["commit", "-m", msg])
            .current_dir(dir)
            .output()
            .expect("git commit");
    }

    // ── E1: base_branch 指定時に正しく分岐されること ──────────

    #[test]
    fn test_worktree_add_with_base_branch_branches_correctly() {
        let repo = init_repo();
        let repo_path = repo.path();

        // base branch を作成して commit を追加
        StdCommand::new("git")
            .args(["branch", "base/test"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        let result = worktree_add(repo_path, "wt-base", "task/from-base", Some("base/test"));
        assert!(
            result.is_ok(),
            "worktree_add with base_branch should succeed: {:?}",
            result
        );

        // 作成された worktree が base branch の履歴を持つことを確認
        let wt_path = repo_path.join(".worktrees").join("wt-base");
        let log_out = StdCommand::new("git")
            .args(["log", "--oneline"])
            .current_dir(&wt_path)
            .output()
            .expect("git log");
        let log_text = String::from_utf8_lossy(&log_out.stdout);
        assert!(
            log_text.contains("initial"),
            "worktree should contain initial commit, got: {log_text}"
        );
    }

    // ── E1: 存在しない base_branch 指定時のエラー ──────────

    #[test]
    fn test_worktree_add_nonexistent_base_branch_errors() {
        let repo = init_repo();
        let result = worktree_add(
            repo.path(),
            "wt-bad",
            "task/bad",
            Some("nonexistent/branch"),
        );
        assert!(result.is_err(), "nonexistent base_branch should error");
    }

    // ── E1: 既存 branch 衝突時はエラー ───────────────────────

    #[test]
    fn test_worktree_add_branch_collision_errors() {
        let repo = init_repo();
        let repo_path = repo.path();

        // 先に base/topic を作成
        StdCommand::new("git")
            .args(["branch", "base/topic"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        // task/collision を事前に作成
        StdCommand::new("git")
            .args(["branch", "task/collision"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        let result = worktree_add(repo_path, "wt-coll", "task/collision", Some("base/topic"));
        assert!(result.is_err(), "branch collision should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("branch already exists"),
            "error should mention 'branch already exists', got: {err_msg}"
        );
    }

    // ── E1: base_branch が他 worktree で dirty の場合デフォルトでエラー ──
    // worktree_add_impl を allow_dirty=false で直接呼ぶことで ENV 変更なしにテスト

    #[test]
    fn test_worktree_add_dirty_base_branch_errors_by_default() {
        let repo = init_repo();
        let repo_path = repo.path();

        // topic/dirty を worktree として作成
        worktree_add_impl(repo_path, "topic-dirty", "topic/dirty", None, false)
            .expect("create topic worktree");

        // topic/dirty worktree に未コミット変更を置く
        let dirty_wt = repo_path.join(".worktrees").join("topic-dirty");
        std::fs::write(dirty_wt.join("dirty_file.txt"), "dirty").unwrap();

        let result = worktree_add_impl(
            repo_path,
            "subtask-from-dirty",
            "task/from-dirty",
            Some("topic/dirty"),
            false,
        );
        assert!(result.is_err(), "dirty base_branch should error by default");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("uncommitted changes"),
            "error should mention uncommitted changes, got: {err_msg}"
        );
    }

    // ── E1: allow_dirty=true で dirty でも通過 ──
    // worktree_add_impl を allow_dirty=true で直接呼ぶことで ENV 変更なしにテスト

    #[test]
    fn test_worktree_add_dirty_base_branch_allowed_by_env() {
        let repo = init_repo();
        let repo_path = repo.path();

        worktree_add_impl(repo_path, "topic-dirty2", "topic/dirty2", None, false)
            .expect("create topic worktree");

        let dirty_wt = repo_path.join(".worktrees").join("topic-dirty2");
        std::fs::write(dirty_wt.join("dirty_file2.txt"), "dirty").unwrap();

        let result = worktree_add_impl(
            repo_path,
            "subtask-from-dirty2",
            "task/from-dirty2",
            Some("topic/dirty2"),
            true, // allow_dirty = GIT_WORKFLOW_ALLOW_DIRTY_BASE=1 相当
        );
        assert!(
            result.is_ok(),
            "allow_dirty=true should allow dirty base, got: {:?}",
            result
        );
    }

    // ── E1: base_branch がどの worktree にも checkout されていない場合は dirty チェックをスキップ ──

    #[test]
    fn test_worktree_add_base_branch_not_checked_out_skips_dirty_check() {
        let repo = init_repo();
        let repo_path = repo.path();

        // base-only は worktree なしで ref のみ作成
        StdCommand::new("git")
            .args(["branch", "base/ref-only"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        // allow_dirty=false でも base が checkout されていなければ dirty チェックをスキップ
        let result = worktree_add_impl(
            repo_path,
            "wt-from-ref",
            "task/from-ref",
            Some("base/ref-only"),
            false,
        );
        assert!(
            result.is_ok(),
            "base_branch not checked out should skip dirty check and succeed: {:?}",
            result
        );
    }

    // ── E1: base_branch が別 worktree で checkout されていても分岐できること ──

    #[test]
    fn test_worktree_add_base_branch_in_another_worktree() {
        let repo = init_repo();
        let repo_path = repo.path();

        // topic/shared を worktree として作成（clean）
        worktree_add_impl(repo_path, "topic-shared", "topic/shared", None, false)
            .expect("create topic/shared worktree");

        // topic/shared worktree は clean のまま → subtask 作成は成功するはず
        let result = worktree_add_impl(
            repo_path,
            "subtask-from-shared",
            "task/from-shared",
            Some("topic/shared"),
            false,
        );
        assert!(
            result.is_ok(),
            "clean base_branch in worktree should allow worktree_add: {:?}",
            result
        );
    }

    // ── E2: merge に working_dir を渡して worktree で merge できること ──

    #[test]
    fn test_merge_with_working_dir_succeeds() {
        let repo = init_repo();
        let repo_path = repo.path();

        // topic worktree を作成
        worktree_add(repo_path, "topic-mt", "topic/mt", None).expect("topic worktree");

        // subtask worktree を topic から分岐
        worktree_add(repo_path, "subtask-mt", "task/subtask-mt", Some("topic/mt"))
            .expect("subtask worktree");

        // subtask worktree に commit を追加
        let subtask_path = repo_path.join(".worktrees").join("subtask-mt");
        add_commit(&subtask_path, "st_file.txt", "st content", "st commit");

        // topic worktree を working_dir として merge
        let topic_path = repo_path.join(".worktrees").join("topic-mt");
        let result = merge(repo_path, "task/subtask-mt", "topic/mt", Some(&topic_path));
        assert!(
            result.is_ok(),
            "merge with working_dir should succeed: {:?}",
            result
        );

        // topic worktree に st commit が存在することを確認
        let log_out = StdCommand::new("git")
            .args(["log", "--oneline"])
            .current_dir(&topic_path)
            .output()
            .expect("git log");
        let log_text = String::from_utf8_lossy(&log_out.stdout);
        assert!(
            log_text.contains("st commit"),
            "topic should contain st commit after merge, got: {log_text}"
        );
    }

    // ── E2: working_dir 指定で HEAD 不一致ならエラー ────────

    #[test]
    fn test_merge_with_working_dir_head_mismatch_errors() {
        let repo = init_repo();
        let repo_path = repo.path();

        worktree_add(repo_path, "topic-ma", "topic/ma", None).expect("topic-ma");
        worktree_add(repo_path, "topic-mb", "topic/mb", None).expect("topic-mb");

        let topic_a_path = repo_path.join(".worktrees").join("topic-ma");
        // topic-a の worktree を指定して into_branch = topic/mb（HEAD は topic/ma）
        let result = merge(repo_path, "topic/mb", "topic/mb", Some(&topic_a_path));
        assert!(result.is_err(), "HEAD mismatch should error");
    }

    // ── E3: branch_delete に working_dir を渡して topic worktree で削除できること ──

    #[test]
    fn test_branch_delete_with_working_dir_succeeds() {
        let repo = init_repo();
        let repo_path = repo.path();

        // topic worktree を作成し、そこに subtask branch を分岐
        worktree_add(repo_path, "topic-bd", "topic/bd", None).expect("topic worktree");
        worktree_add(repo_path, "subtask-bd", "task/subtask-bd", Some("topic/bd"))
            .expect("subtask worktree");

        let subtask_path = repo_path.join(".worktrees").join("subtask-bd");
        add_commit(&subtask_path, "bd.txt", "bd", "bd commit");

        // subtask を topic に merge（topic worktree から）
        let topic_path = repo_path.join(".worktrees").join("topic-bd");
        merge(repo_path, "task/subtask-bd", "topic/bd", Some(&topic_path)).expect("merge");

        // subtask worktree を remove してから branch_delete
        worktree_remove(repo_path, "subtask-bd").expect("worktree remove");

        // repo root (HEAD=main) では subtask branch は未 merge と判定される → エラーになるはず
        let fail = branch_delete(repo_path, "task/subtask-bd", None);
        assert!(
            fail.is_err(),
            "branch_delete at repo root should fail because HEAD=main has no subtask commits"
        );

        // topic worktree を working_dir に指定すれば成功する
        let ok = branch_delete(repo_path, "task/subtask-bd", Some(&topic_path));
        assert!(
            ok.is_ok(),
            "branch_delete with topic working_dir should succeed: {:?}",
            ok
        );
    }

    // ── E3: working_dir 指定で未 merge branch は削除できないこと ──

    #[test]
    fn test_branch_delete_with_working_dir_not_merged_errors() {
        let repo = init_repo();
        let repo_path = repo.path();

        worktree_add(repo_path, "topic-bu", "topic/bu", None).expect("topic worktree");
        worktree_add(repo_path, "subtask-bu", "task/subtask-bu", Some("topic/bu"))
            .expect("subtask worktree");

        let subtask_path = repo_path.join(".worktrees").join("subtask-bu");
        add_commit(&subtask_path, "bu.txt", "bu", "bu commit");

        // 未 merge のまま subtask worktree を remove
        worktree_remove(repo_path, "subtask-bu").expect("worktree remove");

        // topic worktree を working_dir に指定しても subtask は未 merge なので削除不可
        let topic_path = repo_path.join(".worktrees").join("topic-bu");
        let result = branch_delete(repo_path, "task/subtask-bu", Some(&topic_path));
        assert!(
            result.is_err(),
            "branch_delete of not-fully-merged branch should fail"
        );
    }

    // ── E3: working_dir 省略時は repo_root で動作（後方互換） ──

    #[test]
    fn test_branch_delete_without_working_dir_uses_repo_root() {
        let repo = init_repo();
        let repo_path = repo.path();

        // main から直接分岐 → main に merge 可能な branch を作る
        StdCommand::new("git")
            .args(["branch", "feat/compat-bd"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        // main HEAD と同一 commit を指しているので --merged 判定は通る
        let result = branch_delete(repo_path, "feat/compat-bd", None);
        assert!(
            result.is_ok(),
            "branch_delete without working_dir should work on repo root: {:?}",
            result
        );
    }

    // ── M-1: 未登録の working_dir は拒否される ──

    #[test]
    fn test_branch_delete_rejects_unknown_working_dir() {
        let repo = init_repo();
        let repo_path = repo.path();

        // 別の独立した repo を作成（repo の worktree ではない）
        let foreign = init_repo();
        let foreign_path = foreign.path();

        // repo 側に削除対象の branch を用意（main HEAD から分岐）
        StdCommand::new("git")
            .args(["branch", "feat/reject-wd"])
            .current_dir(repo_path)
            .output()
            .expect("git branch");

        // foreign_path は repo の worktree ではないため拒否されるべき
        let result = branch_delete(repo_path, "feat/reject-wd", Some(foreign_path));
        assert!(
            result.is_err(),
            "branch_delete should reject working_dir that is not a known worktree"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(
            msg.contains("neither repo root nor a known worktree"),
            "error should indicate unknown worktree: {msg}"
        );

        // branch は残存しているはず
        assert!(
            branch_exists(repo_path, "feat/reject-wd").unwrap(),
            "branch should not have been deleted"
        );
    }

    // ── E2: working_dir 省略時は repo_root で HEAD チェック ──

    #[test]
    fn test_merge_without_working_dir_uses_repo_root() {
        let repo = init_repo();
        let repo_path = repo.path();

        worktree_add(repo_path, "feat-compat", "task/compat-ut", None).expect("feat worktree");
        let feat_path = repo_path.join(".worktrees").join("feat-compat");
        add_commit(&feat_path, "compat.txt", "compat", "compat commit");

        // repo_root の HEAD は main/master なので task/compat-ut は不一致 → エラー
        let result = merge(repo_path, "task/compat-ut", "task/compat-ut", None);
        assert!(
            result.is_err(),
            "merge without working_dir should check repo root HEAD"
        );
    }

    // ── validate_remote_name ──────────────────────────────

    #[test]
    fn test_validate_remote_name_accept() {
        for name in &["origin", "upstream-1", "my.remote/sub"] {
            assert!(validate_remote_name(name).is_ok(), "should accept: {name}");
        }
    }

    #[test]
    fn test_validate_remote_name_reject() {
        for name in &["a; rm -rf", "origin b", ""] {
            let result = validate_remote_name(name);
            assert!(result.is_err(), "should reject: {name:?}");
            let msg = format!("{:?}", result.unwrap_err());
            assert!(
                msg.contains("invalid remote name"),
                "error should mention 'invalid remote name': {msg}"
            );
        }
    }

    // ── validate_refspec ──────────────────────────────────

    #[test]
    fn test_validate_refspec_accept() {
        for spec in &["refs/heads/main", "+refs/heads/*:refs/remotes/origin/*"] {
            assert!(validate_refspec(spec).is_ok(), "should accept: {spec}");
        }
    }

    #[test]
    fn test_validate_refspec_reject() {
        for spec in &["refs; echo", ""] {
            let result = validate_refspec(spec);
            assert!(result.is_err(), "should reject: {spec:?}");
            let msg = format!("{:?}", result.unwrap_err());
            assert!(
                msg.contains("invalid refspec"),
                "error should mention 'invalid refspec': {msg}"
            );
        }
    }
}
