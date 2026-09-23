use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration not found: {0}")]
    ConfigNotFound(String),

    #[error("configuration already exists: {}", .0.display())]
    ConfigExists(PathBuf),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("failed to parse YAML in {}: {source}", path.display())]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("herdr command failed: {0}")]
    Herdr(String),

    #[error("workspace conflict: {0}")]
    WorkspaceConflict(String),

    #[error("cannot attach without an interactive terminal; use --no-attach")]
    NoTty,

    #[error("cannot attach to Herdr from inside a Herdr pane; use --no-attach")]
    NestedHerdr,

    #[error("no editor configured; set VISUAL or EDITOR")]
    EditorNotConfigured,

    #[error("editor failed: {0}")]
    Editor(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
