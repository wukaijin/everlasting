//! Project-related types shared between `db.rs` and the `projects/`
//! module.

use serde::Serialize;

/// A project as exposed over Tauri IPC. Mirrors a row in the
/// `projects` table (snake_case fields, `camelCase` opt-in is not
/// applied here — the Rust convention of snake_case in the row is
/// kept because the existing `db::SessionRow` / `SessionSummary`
/// already serialize in snake_case and the TypeScript side has
/// TypeScript-side mapping at the store boundary, not the IPC
/// boundary).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_git_repo: bool,
    /// Current branch name, or `None` for non-git projects. The literal
    /// string `"HEAD"` is stored for detached-HEAD repos so the UI can
    /// distinguish detached state from a real branch.
    pub git_branch: Option<String>,
    pub is_legacy: bool,
    pub created_at: String,
    pub updated_at: String,
    pub hidden: bool,
    pub metadata: Option<String>,
}

impl ProjectRow {
    /// Default display name for a project: `basename(path)`.
    pub fn default_name_from_path(path: &str) -> String {
        std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.to_string())
    }
}
