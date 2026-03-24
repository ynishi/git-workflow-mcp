use serde::{Deserialize, Serialize};

/// MCP接続ごとに一意なセッションID。
/// サーバー起動時に生成し、worktree作成時に記録する。
/// destructive操作はこのIDが一致する場合のみ許可。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// worktreeとセッションの紐付けレコード。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSession {
    pub worktree_name: String,
    pub branch: String,
    pub session_id: SessionId,
    pub created_at: String,
}
