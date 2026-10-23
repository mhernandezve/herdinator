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
    let verbose = cli.verbose;
    if !matches!(&cli.command, Command::Init) {
        store.initialize_samples()?;
    }
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
            let mut manager = ProjectManager::new(HerdrCli::new().verbose(verbose));
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
        Command::Init => {
            let created = store.initialize_samples()?;
            if created.is_empty() {
                println!("Samples already exist in {}", store.global_dir().display());
            } else {
                for path in created {
                    println!("Created {}", path.display());
                }
            }
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
            ProjectManager::new(HerdrCli::new().verbose(verbose)).stop(&plan)?;
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
        Command::Doctor => doctor(&store, verbose)?,
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
    // Validate before rewriting so a broken source is not copied silently.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&source_text).map_err(|error| Error::Yaml {
            path: source.to_path_buf(),
            source: error,
        })?;
    if !yaml.is_mapping() {
        return Err(Error::InvalidConfig(
            "project YAML must be a mapping".into(),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target, replace_project_name(&source_text, target_name))?;
    Ok(())
}

/// Rewrites the top-level `name:` scalar in place, leaving comments, ordering,
/// indentation and quoting of everything else untouched.
fn replace_project_name(source: &str, target_name: &str) -> String {
    let scalar = serde_yaml::to_string(target_name)
        .map(|rendered| rendered.trim_end().to_string())
        .unwrap_or_else(|_| format!("{target_name:?}"));

    let mut lines = Vec::new();
    let mut replaced = false;
    for line in source.lines() {
        if !replaced && is_project_name_line(line) {
            let (_, comment) = split_comment(line);
            let suffix = if comment.is_empty() {
                String::new()
            } else {
                format!(" {comment}")
            };
            lines.push(format!("name: {scalar}{suffix}"));
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.insert(0, format!("name: {scalar}"));
    }

    let mut rewritten = lines.join("\n");
    if source.ends_with('\n') {
        rewritten.push('\n');
    }
    rewritten
}

fn is_project_name_line(line: &str) -> bool {
    match line.strip_prefix("name:") {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// Splits a line at a `#` that starts a comment, ignoring hashes inside quotes.
fn split_comment(line: &str) -> (&str, &str) {
    let mut quote = None;
    let mut previous = ' ';
    for (index, character) in line.char_indices() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character == '#' && previous.is_whitespace() => {
                return (&line[..index], &line[index..]);
            }
            None => {}
        }
        previous = character;
    }
    (line, "")
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

fn doctor(store: &ConfigStore, verbose: bool) -> Result<()> {
    println!("Config directory: {}", store.global_dir().display());
    println!(
        "Editor: {}",
        std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "not configured".into())
    );
    let mut herdr = HerdrCli::new().verbose(verbose);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copy_preserves_comments_and_formatting() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.yml");
        let target = dir.path().join("target.yml");
        fs::write(
            &source,
            "# top comment\nname: old  # project name\nroot: /tmp\n\nwindows:\n  - dev:        # inline comment\n",
        )
        .unwrap();

        copy_config(&source, &target, "renamed").unwrap();

        let text = fs::read_to_string(&target).unwrap();
        assert!(text.contains("# top comment"), "{text}");
        assert!(text.contains("name: renamed # project name"), "{text}");
        assert!(text.contains("  - dev:        # inline comment"), "{text}");
        assert!(!text.contains("old"), "{text}");
    }

    #[test]
    fn copy_inserts_a_missing_name() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.yml");
        let target = dir.path().join("target.yml");
        fs::write(&source, "root: /tmp\nwindows:\n  - dev:\n").unwrap();

        copy_config(&source, &target, "renamed").unwrap();

        let text = fs::read_to_string(&target).unwrap();
        assert!(text.starts_with("name: renamed\n"), "{text}");
        assert!(text.contains("root: /tmp"));
    }

    #[test]
    fn copy_quotes_names_that_need_it() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.yml");
        let target = dir.path().join("target.yml");
        fs::write(&source, "name: old\nroot: /tmp\n").unwrap();

        copy_config(&source, &target, "a: b").unwrap();

        let text = fs::read_to_string(&target).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(parsed["name"].as_str(), Some("a: b"));
    }

    #[test]
    fn split_comment_ignores_hashes_inside_quotes() {
        assert_eq!(
            split_comment("name: \"a # b\"  # real comment"),
            ("name: \"a # b\"  ", "# real comment")
        );
        assert_eq!(split_comment("name: plain"), ("name: plain", ""));
    }
}
