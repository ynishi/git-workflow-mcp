use std::path::Path;
use std::process::Command;

use crate::domain::error::DomainError;
use crate::domain::worktree::{
    CommitResult, DiffResult, LogEntry, MergeResult, RepoStatus, ResetResult, Worktree,
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
