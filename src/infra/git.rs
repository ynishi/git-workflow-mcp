use std::path::Path;
use std::process::Command;

use crate::domain::error::DomainError;
use crate::domain::worktree::{
    CommitResult, DiffResult, LogEntry, MergeResult, RepoStatus, Worktree,
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

pub fn worktree_add(repo: &Path, worktree_name: &str, branch: &str) -> Result<String, DomainError> {
    let worktree_path = repo.join(".worktrees").join(worktree_name);
    if worktree_path.exists() {
        return Err(DomainError::WorktreeAlreadyExists(
            worktree_name.to_string(),
        ));
    }

    // ブランチ作成
    let branch_result = run_git(repo, &["branch", branch]);
    if let Err(DomainError::Git(ref msg)) = branch_result
        && !msg.contains("already exists")
    {
        return Err(branch_result.unwrap_err());
    }

    // worktree作成
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

pub fn branch_delete(repo: &Path, branch: &str) -> Result<(), DomainError> {
    run_git(repo, &["branch", "-d", branch]).map_err(|e| match e {
        DomainError::Git(msg) if msg.contains("not found") => {
            DomainError::BranchNotFound(branch.to_string())
        }
        other => other,
    })?;
    Ok(())
}

// ─── Status / Diff ───────────────────────────────────────

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

pub fn diff(working_dir: &Path, staged: bool) -> Result<DiffResult, DomainError> {
    let stat_args = if staged {
        vec!["diff", "--cached", "--stat"]
    } else {
        vec!["diff", "--stat"]
    };
    let diff_args = if staged {
        vec!["diff", "--cached"]
    } else {
        vec!["diff"]
    };

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

// ─── Merge ───────────────────────────────────────────────

pub fn merge(repo: &Path, branch: &str, into_branch: &str) -> Result<MergeResult, DomainError> {
    // 現在のブランチを確認
    let current = run_git(repo, &["branch", "--show-current"])?;
    if current != into_branch {
        return Err(DomainError::Git(format!(
            "must be on branch '{into_branch}' to merge, currently on '{current}'"
        )));
    }

    let output = run_git(
        repo,
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
