use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};

use clap::Parser;
use herdinator::cli::{Cli, Command};
use herdinator::config::{template, ConfigStore, ProjectPlan};
use herdinator::herdr::{HerdrApi, HerdrCli};
use herdinator::project::{ProjectManager, StartOutcome};
use herdinator::{Error, Result};

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let store = ConfigStore::discover()?;
    match cli.command {
        Command::Start {
            project,
            project_config,
            name,
            no_attach,
        } => {
            let path = store.resolve(project.as_deref(), project_config.as_deref())?;
            let plan = ProjectPlan::from_file(&path, name.as_deref())?;
            let attach = plan.attach && !no_attach;
            validate_attach(attach)?;
            let mut manager = ProjectManager::new(HerdrCli::new());
            let outcome = manager.start(&plan, attach)?;
            if !attach {
                match outcome {
                    StartOutcome::Created => println!("Created workspace '{}'", plan.name),
                    StartOutcome::Existing => println!("Workspace '{}' already exists", plan.name),
                }
            }
        }
        Command::New { project, local } => {
            let path = store.path_for_new(&project, local)?;
            create_config(&path, &project)?;
            open_editor(&path)?;
        }
        Command::Open { project } => {
            let path = store
                .existing_named(&project)
                .or_else(|_| store.path_for_new(&project, false))?;
            if !path.exists() {
                create_config(&path, &project)?;
            }
            open_editor(&path)?;
        }
        Command::Edit { project } => {
            open_editor(&store.existing_named(&project)?)?;
        }
        Command::List => {
            let projects = store.list()?;
            if projects.is_empty() {
                println!("No projects found in {}", store.global_dir().display());
            } else {
                for project in projects {
                    println!("{project}");
                }
            }
        }
        Command::Stop { project } => {
            let path = store.resolve(Some(&project), None)?;
            let plan = ProjectPlan::from_file(&path, None)?;
            ProjectManager::new(HerdrCli::new()).stop(&plan)?;
            println!("Stopped workspace '{}'", plan.name);
        }
        Command::Copy { source, target } => {
            let source_path = store.existing_named(&source)?;
            let target_path = store.path_for_new(&target, false)?;
            copy_config(&source_path, &target_path, &target)?;
            println!("Copied '{source}' to '{}'", target_path.display());
        }
        Command::Delete { project } => {
            let path = store.existing_named(&project)?;
            fs::remove_file(&path)?;
            println!("Deleted {}", path.display());
        }
        Command::Debug {
            project,
            project_config,
            name,
        } => {
            let path = store.resolve(project.as_deref(), project_config.as_deref())?;
            let plan = ProjectPlan::from_file(&path, name.as_deref())?;
            let output = serde_yaml::to_string(&plan)
                .map_err(|error| Error::InvalidConfig(error.to_string()))?;
            print!("{output}");
        }
        Command::Doctor => doctor(&store)?,
    }
    Ok(())
}

fn validate_attach(attach: bool) -> Result<()> {
    if !attach {
        return Ok(());
    }
    if std::env::var("HERDR_ENV").as_deref() == Ok("1") {
        return Err(Error::NestedHerdr);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::NoTty);
    }
    Ok(())
}

fn create_config(path: &Path, project: &str) -> Result<()> {
    if path.exists() {
        return Err(Error::ConfigExists(path.to_path_buf()));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, template(project))?;
    Ok(())
}

fn copy_config(source: &Path, target: &Path, target_name: &str) -> Result<()> {
    if target.exists() {
        return Err(Error::ConfigExists(target.to_path_buf()));
    }
    let source_text = fs::read_to_string(source)?;
    let mut yaml: serde_yaml::Value =
        serde_yaml::from_str(&source_text).map_err(|error| Error::Yaml {
            path: source.to_path_buf(),
            source: error,
        })?;
    let mapping = yaml
        .as_mapping_mut()
        .ok_or_else(|| Error::InvalidConfig("project YAML must be a mapping".into()))?;
    mapping.insert(
        serde_yaml::Value::String("name".into()),
        serde_yaml::Value::String(target_name.into()),
    );
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        target,
        serde_yaml::to_string(&yaml).map_err(|error| Error::InvalidConfig(error.to_string()))?,
    )?;
    Ok(())
}

fn open_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .map_err(|_| Error::EditorNotConfigured)?;
    let mut parts = shell_words::split(&editor)
        .map_err(|error| Error::Editor(format!("invalid editor command: {error}")))?;
    if parts.is_empty() {
        return Err(Error::EditorNotConfigured);
    }
    let executable = parts.remove(0);
    let status = ProcessCommand::new(executable)
        .args(parts)
        .arg(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Editor(format!(
            "editor exited with status {}",
            status.code().unwrap_or(-1)
        )))
    }
}

fn doctor(store: &ConfigStore) -> Result<()> {
    println!("Config directory: {}", store.global_dir().display());
    println!(
        "Editor: {}",
        std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "not configured".into())
    );
    let mut herdr = HerdrCli::new();
    println!("Herdr: {}", herdr.version()?);
    let server_running = herdr.server_running();
    println!(
        "Server: {}",
        if server_running { "running" } else { "stopped" }
    );
    if server_running {
        println!("Running workspaces: {}", herdr.list_workspaces()?.len());
    }
    println!("Projects: {}", store.list()?.len());
    Ok(())
}
