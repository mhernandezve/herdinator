use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "herdinator",
    version,
    about = "Create and manage Herdr workspaces from tmuxinator-style YAML"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start or focus a project
    #[command(alias = "s")]
    Start {
        /// Project name; defaults to the current directory name
        project: Option<String>,
        /// Use a configuration file directly
        #[arg(short = 'p', long = "project-config")]
        project_config: Option<PathBuf>,
        /// Override the Herdr workspace name
        #[arg(short = 'n', long = "name")]
        name: Option<String>,
        /// Prepare the workspace without opening Herdr
        #[arg(long)]
        no_attach: bool,
    },
    /// Create a project configuration and open it in $VISUAL or $EDITOR
    #[command(alias = "n")]
    New {
        project: String,
        /// Store the configuration as .herdinator.yml in the current directory
        #[arg(long)]
        local: bool,
    },
    /// Create if needed, then open a project configuration
    #[command(alias = "o")]
    Open { project: String },
    /// Edit an existing project configuration
    #[command(alias = "e")]
    Edit { project: String },
    /// List configured projects
    #[command(alias = "l", alias = "ls")]
    List,
    /// Close a project's Herdr workspace
    Stop { project: String },
    /// Copy a project configuration
    #[command(alias = "c", alias = "cp")]
    Copy { source: String, target: String },
    /// Delete a project configuration
    #[command(alias = "rm")]
    Delete { project: String },
    /// Parse and print the normalized project plan
    Debug {
        project: Option<String>,
        #[arg(short = 'p', long = "project-config")]
        project_config: Option<PathBuf>,
        #[arg(short = 'n', long = "name")]
        name: Option<String>,
    },
    /// Check the local Herdinator and Herdr environment
    Doctor,
}
