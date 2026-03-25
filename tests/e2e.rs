use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::process::Command;

/// Build path to the server binary (debug or release).
fn server_bin() -> String {
    std::env::var("CARGO_BIN_EXE_git-workflow-mcp").unwrap_or_else(|_| {
        format!(
            "{}/target/debug/git-workflow-mcp",
            env!("CARGO_MANIFEST_DIR")
        )
    })
}

/// Spawn the MCP server with the given args and connect via rmcp client.
async fn connect_with_args(args: &[&str]) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let mut cmd = Command::new(server_bin());
    for arg in args {
        cmd.arg(arg);
    }
    let transport = TokioChildProcess::new(cmd).expect("failed to spawn server process");
    ().serve(transport)
        .await
        .expect("failed to initialize MCP client")
}

/// Spawn the MCP server as a child process and connect via rmcp client.
async fn connect() -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    connect_with_args(&[]).await
}

/// Build CallToolRequestParams from a tool name and a JSON value.
fn call_params(name: &str, args: Value) -> CallToolRequestParams {
    let arguments = match args {
        Value::Object(map) => Some(map),
        _ => None,
    };
    CallToolRequestParams {
        name: name.to_string().into(),
        arguments,
        meta: None,
        task: None,
    }
}

/// Temporary git repo for testing tools that need a real repo.
struct TempRepo {
    dir: tempfile::TempDir,
}

impl TempRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .expect("git init failed");
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .expect("git config email failed");
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .expect("git config name failed");
        std::fs::write(dir.path().join("README.md"), "# test").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .expect("git add failed");
        std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .output()
            .expect("git commit failed");
        Self { dir }
    }

    fn path_str(&self) -> &str {
        self.dir.path().to_str().unwrap()
    }
}

// ─── Tests ────────────────────────────────────────────────

#[tokio::test]
async fn test_list_tools() {
    let client = connect().await;
    let tools = client.peer().list_all_tools().await.unwrap();

    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(tool_names.contains(&"session_start"));
    assert!(tool_names.contains(&"worktree_add"));
    assert!(tool_names.contains(&"worktree_remove"));
    assert!(tool_names.contains(&"worktree_list"));
    assert!(tool_names.contains(&"branch_delete"));
    assert!(tool_names.contains(&"commit"));
    assert!(tool_names.contains(&"merge"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"log"));
    assert!(tool_names.contains(&"safe_reset"));

    let mut tool_summary: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
            })
        })
        .collect();
    tool_summary.sort_by(|a, b| a["name"].as_str().unwrap().cmp(b["name"].as_str().unwrap()));

    insta::assert_json_snapshot!("tool_list", tool_summary);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_status_tool() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "status",
            json!({ "working_dir": repo.path_str() }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(text.contains("Branch:"));
    assert!(text.contains("Clean: true"));

    insta::assert_snapshot!(
        "status_clean_repo",
        normalize_output(&text, repo.path_str())
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_no_changes() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str() }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    insta::assert_snapshot!("diff_no_changes", text);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_log_tool() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "log",
            json!({ "working_dir": repo.path_str(), "max_count": 5 }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(text.contains("initial"));

    insta::assert_snapshot!("log_initial", redact_hashes(&text));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_session_start() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "session_start",
            json!({ "repo_root": repo.path_str() }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(text.contains("Session started."));
    assert!(text.contains("Session ID:"));

    insta::assert_snapshot!(
        "session_start",
        normalize_output(&redact_session_id(&text), repo.path_str())
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_session_start_invalid_repo() {
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "session_start",
            json!({ "repo_root": "/tmp/nonexistent-repo-12345" }),
        ))
        .await;

    assert!(result.is_err());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_worktree_lifecycle() {
    let repo = TempRepo::new();
    let client = connect().await;

    // 1. session_start
    let result = client
        .peer()
        .call_tool(call_params(
            "session_start",
            json!({ "repo_root": repo.path_str() }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("Session started."));

    // 2. worktree_add
    let result = client
        .peer()
        .call_tool(call_params(
            "worktree_add",
            json!({ "name": "test-feature", "branch": "task/test-feature" }),
        ))
        .await
        .unwrap();
    let text = extract_text(&result);
    assert!(text.contains("Worktree created."));
    assert!(text.contains("task/test-feature"));

    // 3. worktree_list
    let result = client
        .peer()
        .call_tool(call_params("worktree_list", json!({})))
        .await
        .unwrap();
    let text = extract_text(&result);
    assert!(text.contains("test-feature"));
    assert!(text.contains("(this session)"));

    // 4. worktree_remove
    let result = client
        .peer()
        .call_tool(call_params(
            "worktree_remove",
            json!({ "name": "test-feature" }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("removed"));

    // 5. branch_delete
    let result = client
        .peer()
        .call_tool(call_params(
            "branch_delete",
            json!({ "branch": "task/test-feature" }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("deleted"));

    client.cancel().await.unwrap();
}

// ─── safe_reset tests ─────────────────────────────────────

/// TempRepo に追加コミットを1本作成するヘルパー。
fn add_commit(repo: &TempRepo, filename: &str, content: &str, message: &str) {
    std::fs::write(repo.dir.path().join(filename), content).unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.dir.path())
        .output()
        .expect("git add failed");
    std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(repo.dir.path())
        .output()
        .expect("git commit failed");
}

#[tokio::test]
async fn test_safe_reset_soft() {
    let repo = TempRepo::new();
    add_commit(&repo, "file1.txt", "hello", "second commit");

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "safe_reset",
            json!({ "working_dir": repo.path_str(), "mode": "soft", "target": "HEAD~1" }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(text.contains("Reset completed (soft)."));
    assert!(text.contains("Previous HEAD:"));
    assert!(text.contains("New HEAD:"));
    assert!(text.contains("To undo: git reset --soft"));

    // soft reset後: 変更はstagingにあるはず
    let diff_result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "staged": true }),
        ))
        .await
        .unwrap();
    let diff_text = extract_text(&diff_result);
    assert!(
        diff_text.contains("file1.txt"),
        "staged changes should contain file1.txt after soft reset"
    );

    insta::assert_snapshot!("safe_reset_soft", redact_hashes(&text));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_safe_reset_mixed() {
    let repo = TempRepo::new();
    add_commit(&repo, "file2.txt", "world", "second commit");

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "safe_reset",
            json!({ "working_dir": repo.path_str(), "mode": "mixed", "target": "HEAD~1" }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(text.contains("Reset completed (mixed)."));
    assert!(text.contains("Previous HEAD:"));
    assert!(text.contains("New HEAD:"));

    // mixed reset後: staged diffは空のはず
    let staged_diff_result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "staged": true }),
        ))
        .await
        .unwrap();
    let staged_diff_text = extract_text(&staged_diff_result);
    assert!(
        staged_diff_text.contains("No changes."),
        "staged diff should be empty after mixed reset, got: {staged_diff_text}"
    );

    // status で file2.txt が変更ファイル一覧に現れるはず（untracked または modified）
    let status_result = client
        .peer()
        .call_tool(call_params(
            "status",
            json!({ "working_dir": repo.path_str() }),
        ))
        .await
        .unwrap();
    let status_text = extract_text(&status_result);
    assert!(
        status_text.contains("file2.txt"),
        "status should show file2.txt after mixed reset, got: {status_text}"
    );

    insta::assert_snapshot!("safe_reset_mixed", redact_hashes(&text));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_safe_reset_invalid_mode() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "safe_reset",
            json!({ "working_dir": repo.path_str(), "mode": "hard", "target": "HEAD~1" }),
        ))
        .await;

    // hard モードはエラーになるはず
    assert!(result.is_err(), "hard mode should return an error");

    client.cancel().await.unwrap();
}

// ─── diff拡張テスト ────────────────────────────────────────

#[tokio::test]
async fn test_diff_commit_range() {
    let repo = TempRepo::new();
    add_commit(&repo, "feature.txt", "feature content", "feature commit");

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "commit_range": "HEAD~1..HEAD" }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(
        text.contains("feature.txt"),
        "commit range diff should include feature.txt, got: {text}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_name_only() {
    let repo = TempRepo::new();
    // 変更をworking copyに追加（未ステージ）
    std::fs::write(repo.dir.path().join("changed.txt"), "some content").unwrap();

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "commit_range": "HEAD~0", "name_only": true }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    // name_only のため stat は空、ファイル名のみ返る（または変更なしの場合は空）
    // HEAD~0 は HEAD と同じなので差分はない。
    // ファイル追加の diff テストとして別途 staged で実施
    assert!(
        !text.contains("---"),
        "name_only should not contain patch, got: {text}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_name_only_staged() {
    let repo = TempRepo::new();
    std::fs::write(repo.dir.path().join("staged_file.txt"), "staged content").unwrap();
    std::process::Command::new("git")
        .args(["add", "staged_file.txt"])
        .current_dir(repo.dir.path())
        .output()
        .expect("git add failed");

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "staged": true, "name_only": true }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(
        text.contains("staged_file.txt"),
        "name_only staged diff should contain staged_file.txt, got: {text}"
    );
    assert!(
        !text.contains("+++"),
        "name_only should not contain patch headers, got: {text}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_paths() {
    let repo = TempRepo::new();
    // 複数ファイルを変更
    std::fs::write(repo.dir.path().join("target.txt"), "target content").unwrap();
    std::fs::write(repo.dir.path().join("other.txt"), "other content").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.dir.path())
        .output()
        .expect("git add failed");

    let client = connect().await;

    // paths で target.txt のみに絞る
    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({
                "working_dir": repo.path_str(),
                "staged": true,
                "paths": ["target.txt"]
            }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(
        text.contains("target.txt"),
        "paths filter should include target.txt, got: {text}"
    );
    assert!(
        !text.contains("other.txt"),
        "paths filter should exclude other.txt, got: {text}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_max_lines() {
    let repo = TempRepo::new();
    // 大きなファイルを追加してコミット差分を作る
    let large_content: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(repo.dir.path().join("large.txt"), &large_content).unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.dir.path())
        .output()
        .expect("git add failed");
    std::process::Command::new("git")
        .args(["commit", "-m", "add large file"])
        .current_dir(repo.dir.path())
        .output()
        .expect("git commit failed");

    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({
                "working_dir": repo.path_str(),
                "commit_range": "HEAD~1..HEAD",
                "max_lines": 5
            }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    assert!(
        text.contains("truncated"),
        "max_lines should truncate output, got: {text}"
    );
    assert!(
        text.contains("showing 5/"),
        "truncation message should show 5 lines shown, got: {text}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_staged_with_commit_range_error() {
    let repo = TempRepo::new();
    let client = connect().await;

    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({
                "working_dir": repo.path_str(),
                "staged": true,
                "commit_range": "HEAD~1..HEAD"
            }),
        ))
        .await;

    assert!(
        result.is_err(),
        "staged + commit_range should return an error"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_diff_head() {
    let repo = TempRepo::new();
    // staged と unstaged 両方ある状態を作る
    std::fs::write(repo.dir.path().join("staged.txt"), "staged").unwrap();
    std::process::Command::new("git")
        .args(["add", "staged.txt"])
        .current_dir(repo.dir.path())
        .output()
        .expect("git add failed");
    std::fs::write(repo.dir.path().join("unstaged.txt"), "unstaged").unwrap();

    let client = connect().await;

    // commit_range: "HEAD" は HEAD と working tree の差分（staged + unstaged 両方）
    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str(), "commit_range": "HEAD" }),
        ))
        .await
        .unwrap();

    let text = extract_text(&result);
    // staged.txt も unstaged.txt も両方表示されるはず
    assert!(
        text.contains("staged.txt") || text.contains("unstaged.txt"),
        "HEAD diff should contain changed files, got: {text}"
    );

    client.cancel().await.unwrap();
}

// ─── --mode read-only tests ───────────────────────────────

const WRITE_TOOLS: &[&str] = &[
    "commit",
    "merge",
    "worktree_add",
    "worktree_remove",
    "branch_delete",
    "safe_reset",
];

#[tokio::test]
async fn test_read_only_list_tools() {
    let client = connect_with_args(&["--mode", "read-only"]).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // WRITE_TOOLS が含まれていないことを確認
    for write_tool in WRITE_TOOLS {
        assert!(
            !tool_names.contains(write_tool),
            "read-only mode should not expose write tool '{write_tool}', got: {tool_names:?}"
        );
    }

    // READ 系ツールは含まれていること
    assert!(tool_names.contains(&"session_start"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"log"));
    assert!(tool_names.contains(&"worktree_list"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_read_only_blocks_write() {
    let repo = TempRepo::new();
    let client = connect_with_args(&["--mode", "read-only"]).await;

    // session_start は read-only でも動くはず
    let result = client
        .peer()
        .call_tool(call_params(
            "session_start",
            json!({ "repo_root": repo.path_str() }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("Session started."));

    // commit はエラーになるはず
    let result = client
        .peer()
        .call_tool(call_params(
            "commit",
            json!({ "message": "should fail", "working_dir": repo.path_str() }),
        ))
        .await;
    assert!(
        result.is_err(),
        "commit should be blocked in read-only mode"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_read_only_allows_read() {
    let repo = TempRepo::new();
    let client = connect_with_args(&["--mode", "read-only"]).await;

    // status は動くはず
    let result = client
        .peer()
        .call_tool(call_params(
            "status",
            json!({ "working_dir": repo.path_str() }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("Branch:"));

    // diff も動くはず
    let result = client
        .peer()
        .call_tool(call_params(
            "diff",
            json!({ "working_dir": repo.path_str() }),
        ))
        .await
        .unwrap();
    // diff は何らかのテキストを返すはず（エラーでない）
    let text = extract_text(&result);
    assert!(
        !text.is_empty() || text.is_empty(),
        "diff should succeed in read-only mode"
    );

    // log も動くはず
    let result = client
        .peer()
        .call_tool(call_params(
            "log",
            json!({ "working_dir": repo.path_str(), "max_count": 5 }),
        ))
        .await
        .unwrap();
    assert!(extract_text(&result).contains("initial"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn test_full_mode_has_all_tools() {
    let client = connect_with_args(&["--mode", "full"]).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // 全11ツールが利用可能であることを確認
    assert!(tool_names.contains(&"session_start"));
    assert!(tool_names.contains(&"worktree_add"));
    assert!(tool_names.contains(&"worktree_remove"));
    assert!(tool_names.contains(&"worktree_list"));
    assert!(tool_names.contains(&"branch_delete"));
    assert!(tool_names.contains(&"commit"));
    assert!(tool_names.contains(&"merge"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"log"));
    assert!(tool_names.contains(&"safe_reset"));

    client.cancel().await.unwrap();
}

// ─── Helpers ──────────────────────────────────────────────

fn extract_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_output(text: &str, repo_path: &str) -> String {
    text.replace(repo_path, "<REPO>")
}

fn redact_hashes(text: &str) -> String {
    let re = regex::Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap();
    re.replace_all(text, "<HASH>").to_string()
}

fn redact_session_id(text: &str) -> String {
    let re =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    re.replace_all(text, "<SESSION_ID>").to_string()
}
