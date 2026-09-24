use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::error::{Error, Result};
use crate::layout::{preset, Direction, LayoutNode, PanePlan};

pub const LOCAL_CONFIG: &str = ".herdinator.yml";
pub const TMUXINATOR_LOCAL_CONFIG: &str = ".tmuxinator.yml";

const SAMPLES: [(&str, &str); 2] = [
    ("tmuxinator.yml", include_str!("../examples/tmuxinator.yml")),
    ("native.yml", include_str!("../examples/native.yml")),
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Selector {
    Index(usize),
    Name(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectPlan {
    pub name: String,
    pub root: PathBuf,
    pub attach: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_window: Option<Selector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_pane: Option<Selector>,
    pub tabs: Vec<TabPlan>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TabPlan {
    pub name: String,
    pub root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_pane: Option<Selector>,
    pub layout: LayoutNode,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProject {
    name: String,
    root: Option<String>,
    pre_window: Option<Value>,
    startup_window: Option<Selector>,
    startup_pane: Option<Selector>,
    attach: Option<bool>,
    windows: Option<Vec<Value>>,
    tabs: Option<Vec<RawNativeTab>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWindowBody {
    root: Option<String>,
    layout: Option<String>,
    panes: Option<Vec<Value>>,
    focused_pane: Option<Selector>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNativeTab {
    name: String,
    root: Option<String>,
    focused_pane: Option<Selector>,
    layout: Value,
}

impl ProjectPlan {
    pub fn from_file(path: &Path, name_override: Option<&str>) -> Result<Self> {
        let source = fs::read_to_string(path)?;
        let raw: RawProject = serde_yaml::from_str(&source).map_err(|source| Error::Yaml {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_raw(raw, name_override)
    }

    fn from_raw(raw: RawProject, name_override: Option<&str>) -> Result<Self> {
        let name = name_override.unwrap_or(&raw.name).trim().to_string();
        if name.is_empty() {
            return Err(Error::InvalidConfig("project name cannot be empty".into()));
        }

        match (&raw.windows, &raw.tabs) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidConfig(
                    "use either 'windows' or 'tabs', not both".into(),
                ))
            }
            (None, None) => {
                return Err(Error::InvalidConfig(
                    "configuration must contain 'windows' or 'tabs'".into(),
                ))
            }
            _ => {}
        }

        let root = expand_directory(raw.root.as_deref().unwrap_or("~"), "project root")?;
        let pre_window = raw
            .pre_window
            .as_ref()
            .map(parse_commands)
            .transpose()?
            .unwrap_or_default();

        let tabs = if let Some(windows) = raw.windows {
            windows
                .into_iter()
                .enumerate()
                .map(|(index, window)| parse_window(index, window, &root, &pre_window))
                .collect::<Result<Vec<_>>>()?
        } else {
            raw.tabs
                .unwrap_or_default()
                .into_iter()
                .map(|tab| parse_native_tab(tab, &root, &pre_window))
                .collect::<Result<Vec<_>>>()?
        };

        if tabs.is_empty() {
            return Err(Error::InvalidConfig(
                "project must contain at least one window".into(),
            ));
        }
        validate_selector(
            "startup_window",
            raw.startup_window.as_ref(),
            &tabs,
            |tab| &tab.name,
        )?;
        let mut tab_names = BTreeSet::new();
        if let Some(duplicate) = tabs
            .iter()
            .map(|tab| tab.name.as_str())
            .find(|name| !tab_names.insert(*name))
        {
            return Err(Error::InvalidConfig(format!(
                "duplicate window name '{duplicate}'"
            )));
        }
        let startup_tab = selector_index(raw.startup_window.as_ref(), &tabs);
        let startup_panes = collect_panes(&tabs[startup_tab].layout);
        validate_pane_selector("startup window", raw.startup_pane.as_ref(), &startup_panes)?;

        Ok(Self {
            name,
            root,
            attach: raw.attach.unwrap_or(true),
            startup_window: raw.startup_window,
            startup_pane: raw.startup_pane,
            tabs,
        })
    }
}

fn parse_window(
    index: usize,
    value: Value,
    project_root: &Path,
    pre_window: &[String],
) -> Result<TabPlan> {
    let mapping = expect_single_mapping(value, "window")?;
    let (key, value) = mapping.into_iter().next().unwrap();
    let name = match key {
        Value::String(name) if !name.trim().is_empty() => name,
        Value::Null => format!("window-{}", index + 1),
        _ => {
            return Err(Error::InvalidConfig(
                "window names must be non-empty strings or null".into(),
            ))
        }
    };

    if value.is_mapping() {
        let body: RawWindowBody = serde_yaml::from_value(value)
            .map_err(|error| Error::InvalidConfig(format!("invalid window '{name}': {error}")))?;
        let root = match body.root {
            Some(root) => expand_directory(&root, &format!("root for window '{name}'"))?,
            None => project_root.to_path_buf(),
        };
        let panes = match body.panes {
            Some(values) if values.is_empty() => {
                return Err(Error::InvalidConfig(format!(
                    "window '{name}' must contain at least one pane"
                )))
            }
            Some(values) => values
                .into_iter()
                .map(|pane| parse_pane(pane, pre_window))
                .collect::<Result<Vec<_>>>()?,
            None => vec![PanePlan {
                name: None,
                commands: pre_window.to_vec(),
            }],
        };
        validate_unique_pane_names(&name, &panes)?;
        validate_pane_selector(&name, body.focused_pane.as_ref(), &panes)?;
        Ok(TabPlan {
            name,
            root,
            focused_pane: body.focused_pane,
            layout: preset(body.layout.as_deref(), panes)?,
        })
    } else {
        let mut commands = pre_window.to_vec();
        commands.extend(parse_commands(&value)?);
        Ok(TabPlan {
            name,
            root: project_root.to_path_buf(),
            focused_pane: None,
            layout: LayoutNode::pane(PanePlan {
                name: None,
                commands,
            }),
        })
    }
}

fn parse_pane(value: Value, pre_window: &[String]) -> Result<PanePlan> {
    let (name, commands) = if value.is_mapping() {
        let mapping = expect_single_mapping(value, "pane")?;
        let (key, value) = mapping.into_iter().next().unwrap();
        let name = match key {
            Value::String(name) if !name.trim().is_empty() => Some(name),
            Value::Null => None,
            _ => {
                return Err(Error::InvalidConfig(
                    "pane names must be non-empty strings or null".into(),
                ))
            }
        };
        (name, parse_commands(&value)?)
    } else {
        (None, parse_commands(&value)?)
    };

    let mut all_commands = pre_window.to_vec();
    all_commands.extend(commands);
    Ok(PanePlan {
        name,
        commands: all_commands,
    })
}

fn parse_commands(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Null => Ok(vec![]),
        Value::String(command) => Ok((!command.trim().is_empty())
            .then(|| command.clone())
            .into_iter()
            .collect()),
        Value::Sequence(commands) => commands
            .iter()
            .map(|command| match command {
                Value::String(command) if !command.trim().is_empty() => Ok(command.clone()),
                Value::String(_) | Value::Null => Ok(String::new()),
                _ => Err(Error::InvalidConfig(
                    "commands must be strings or null".into(),
                )),
            })
            .filter_map(|result| match result {
                Ok(command) if command.is_empty() => None,
                other => Some(other),
            })
            .collect(),
        _ => Err(Error::InvalidConfig(
            "a command must be a string, a list of strings, or null".into(),
        )),
    }
}

fn parse_native_tab(
    tab: RawNativeTab,
    project_root: &Path,
    pre_window: &[String],
) -> Result<TabPlan> {
    if tab.name.trim().is_empty() {
        return Err(Error::InvalidConfig("tab name cannot be empty".into()));
    }
    let root = match tab.root {
        Some(root) => expand_directory(&root, &format!("root for tab '{}'", tab.name))?,
        None => project_root.to_path_buf(),
    };
    let mut layout = parse_native_node(tab.layout)?;
    prepend_commands(&mut layout, pre_window);
    let panes = collect_panes(&layout);
    validate_unique_pane_names(&tab.name, &panes)?;
    validate_pane_selector(&tab.name, tab.focused_pane.as_ref(), &panes)?;
    Ok(TabPlan {
        name: tab.name,
        root,
        focused_pane: tab.focused_pane,
        layout,
    })
}

fn parse_native_node(value: Value) -> Result<LayoutNode> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| Error::InvalidConfig("native layout nodes must be mappings".into()))?;
    let keys = string_keys(mapping)?;

    if keys.contains("split") {
        reject_unknown(&keys, &["split", "ratio", "panes"], "split node")?;
        let direction: Direction = serde_yaml::from_value(
            mapping
                .get(Value::String("split".into()))
                .cloned()
                .ok_or_else(|| Error::InvalidConfig("split direction is required".into()))?,
        )
        .map_err(|_| Error::InvalidConfig("split must be 'right' or 'down'".into()))?;
        let ratio = mapping
            .get(Value::String("ratio".into()))
            .map(|value| serde_yaml::from_value::<f64>(value.clone()))
            .transpose()
            .map_err(|_| Error::InvalidConfig("split ratio must be a number".into()))?
            .unwrap_or(0.5);
        if !(0.05..=0.95).contains(&ratio) {
            return Err(Error::InvalidConfig(
                "split ratio must be between 0.05 and 0.95".into(),
            ));
        }
        let panes = mapping
            .get(Value::String("panes".into()))
            .and_then(Value::as_sequence)
            .ok_or_else(|| Error::InvalidConfig("split node requires two panes".into()))?;
        if panes.len() != 2 {
            return Err(Error::InvalidConfig(
                "split node must contain exactly two panes".into(),
            ));
        }
        Ok(LayoutNode::Split {
            direction,
            ratio,
            first: Box::new(parse_native_node(panes[0].clone())?),
            second: Box::new(parse_native_node(panes[1].clone())?),
        })
    } else {
        reject_unknown(&keys, &["name", "command", "commands"], "pane node")?;
        if keys.contains("command") && keys.contains("commands") {
            return Err(Error::InvalidConfig(
                "native pane cannot use both 'command' and 'commands'".into(),
            ));
        }
        let name = mapping
            .get(Value::String("name".into()))
            .map(|value| match value {
                Value::String(name) if !name.trim().is_empty() => Ok(name.clone()),
                _ => Err(Error::InvalidConfig(
                    "native pane name must be a non-empty string".into(),
                )),
            })
            .transpose()?;
        let command = mapping
            .get(Value::String("command".into()))
            .or_else(|| mapping.get(Value::String("commands".into())));
        let commands = command.map(parse_commands).transpose()?.unwrap_or_default();
        Ok(LayoutNode::pane(PanePlan { name, commands }))
    }
}

fn prepend_commands(node: &mut LayoutNode, commands: &[String]) {
    match node {
        LayoutNode::Pane { pane } => {
            let mut all = commands.to_vec();
            all.append(&mut pane.commands);
            pane.commands = all;
        }
        LayoutNode::Split { first, second, .. } => {
            prepend_commands(first, commands);
            prepend_commands(second, commands);
        }
    }
}

fn collect_panes(node: &LayoutNode) -> Vec<PanePlan> {
    match node {
        LayoutNode::Pane { pane } => vec![pane.clone()],
        LayoutNode::Split { first, second, .. } => {
            let mut panes = collect_panes(first);
            panes.extend(collect_panes(second));
            panes
        }
    }
}

fn validate_pane_selector(
    window: &str,
    selector: Option<&Selector>,
    panes: &[PanePlan],
) -> Result<()> {
    let Some(selector) = selector else {
        return Ok(());
    };
    match selector {
        Selector::Index(index) if *index < panes.len() => Ok(()),
        Selector::Name(name) if panes.iter().any(|pane| pane.name.as_deref() == Some(name)) => {
            Ok(())
        }
        _ => Err(Error::InvalidConfig(format!(
            "focused_pane in window '{window}' does not identify a pane"
        ))),
    }
}

fn validate_unique_pane_names(window: &str, panes: &[PanePlan]) -> Result<()> {
    let mut names = BTreeSet::new();
    if let Some(duplicate) = panes
        .iter()
        .filter_map(|pane| pane.name.as_deref())
        .find(|name| !names.insert(*name))
    {
        return Err(Error::InvalidConfig(format!(
            "duplicate pane name '{duplicate}' in window '{window}'"
        )));
    }
    Ok(())
}

fn selector_index(selector: Option<&Selector>, tabs: &[TabPlan]) -> usize {
    match selector {
        None => 0,
        Some(Selector::Index(index)) => *index,
        Some(Selector::Name(name)) => tabs
            .iter()
            .position(|tab| &tab.name == name)
            .expect("startup_window was validated"),
    }
}

fn validate_selector<F>(
    field: &str,
    selector: Option<&Selector>,
    tabs: &[TabPlan],
    name: F,
) -> Result<()>
where
    F: Fn(&TabPlan) -> &String,
{
    let Some(selector) = selector else {
        return Ok(());
    };
    let valid = match selector {
        Selector::Index(index) => *index < tabs.len(),
        Selector::Name(expected) => tabs.iter().any(|tab| name(tab) == expected),
    };
    valid.then_some(()).ok_or_else(|| {
        Error::InvalidConfig(format!("{field} does not identify a configured window"))
    })
}

fn expect_single_mapping(value: Value, kind: &str) -> Result<Mapping> {
    let mapping = match value {
        Value::Mapping(mapping) => mapping,
        _ => {
            return Err(Error::InvalidConfig(format!(
                "each {kind} must be a single-key mapping"
            )))
        }
    };
    if mapping.len() != 1 {
        return Err(Error::InvalidConfig(format!(
            "each {kind} must contain exactly one name"
        )));
    }
    Ok(mapping)
}

fn string_keys(mapping: &Mapping) -> Result<BTreeSet<String>> {
    mapping
        .keys()
        .map(|key| match key {
            Value::String(key) => Ok(key.clone()),
            _ => Err(Error::InvalidConfig(
                "native layout keys must be strings".into(),
            )),
        })
        .collect()
}

fn reject_unknown(keys: &BTreeSet<String>, allowed: &[&str], kind: &str) -> Result<()> {
    if let Some(key) = keys.iter().find(|key| !allowed.contains(&key.as_str())) {
        return Err(Error::InvalidConfig(format!(
            "unsupported field '{key}' in {kind}"
        )));
    }
    Ok(())
}

fn expand_directory(value: &str, field: &str) -> Result<PathBuf> {
    let expanded = shellexpand::full(value)
        .map_err(|error| Error::InvalidConfig(format!("cannot expand {field}: {error}")))?;
    let path = PathBuf::from(expanded.as_ref());
    if !path.is_dir() {
        return Err(Error::InvalidConfig(format!(
            "{field} is not a directory: {}",
            path.display()
        )));
    }
    Ok(path.canonicalize().unwrap_or(path))
}

pub struct ConfigStore {
    global_dir: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self> {
        let global_dir = if let Some(path) = std::env::var_os("HERDINATOR_CONFIG") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(path).join("herdinator")
        } else if let Some(home) = dirs::home_dir() {
            home.join(".config").join("herdinator")
        } else {
            return Err(Error::ConfigNotFound(
                "could not determine the configuration directory".into(),
            ));
        };
        Ok(Self { global_dir })
    }

    pub fn global_dir(&self) -> &Path {
        &self.global_dir
    }

    pub fn resolve(&self, project: Option<&str>, explicit: Option<&Path>) -> Result<PathBuf> {
        if let Some(path) = explicit {
            return path
                .is_file()
                .then(|| path.to_path_buf())
                .ok_or_else(|| Error::ConfigNotFound(path.display().to_string()));
        }

        if let Some(project) = project {
            if let Some(path) = self.named_path(project) {
                return Ok(path);
            }
        }

        for name in [LOCAL_CONFIG, TMUXINATOR_LOCAL_CONFIG] {
            let path = PathBuf::from(name);
            if path.is_file() {
                return Ok(path);
            }
        }

        let inferred = project.map(str::to_owned).or_else(|| {
            std::env::current_dir()
                .ok()?
                .file_name()?
                .to_str()
                .map(str::to_owned)
        });
        if let Some(project) = inferred {
            if let Some(path) = self.named_path(&project) {
                return Ok(path);
            }
        }

        Err(Error::ConfigNotFound(
            project.unwrap_or("current directory").to_string(),
        ))
    }

    pub fn path_for_new(&self, project: &str, local: bool) -> Result<PathBuf> {
        validate_project_name(project)?;
        if local {
            Ok(PathBuf::from(LOCAL_CONFIG))
        } else {
            Ok(self.global_dir.join(format!("{project}.yml")))
        }
    }

    pub fn existing_named(&self, project: &str) -> Result<PathBuf> {
        self.named_path(project)
            .ok_or_else(|| Error::ConfigNotFound(project.into()))
    }

    pub fn list(&self) -> Result<Vec<String>> {
        if !self.global_dir.is_dir() {
            return Ok(vec![]);
        }
        let mut projects = BTreeSet::new();
        for entry in fs::read_dir(&self.global_dir)? {
            let path = entry?.path();
            if path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("yml" | "yaml")
                )
            {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    projects.insert(stem.to_string());
                }
            }
        }
        Ok(projects.into_iter().collect())
    }

    pub fn initialize_samples(&self) -> Result<Vec<PathBuf>> {
        fs::create_dir_all(&self.global_dir)?;
        let mut created = Vec::new();
        for (name, contents) in SAMPLES {
            let path = self.global_dir.join(name);
            if !path.exists() {
                fs::write(&path, contents)?;
                created.push(path);
            }
        }
        Ok(created)
    }

    fn named_path(&self, project: &str) -> Option<PathBuf> {
        ["yml", "yaml"]
            .into_iter()
            .map(|extension| self.global_dir.join(format!("{project}.{extension}")))
            .find(|path| path.is_file())
    }
}

pub fn validate_project_name(project: &str) -> Result<()> {
    if project.is_empty()
        || project.contains('.')
        || project.contains('/')
        || project.contains('\\')
    {
        return Err(Error::InvalidConfig(format!(
            "invalid project name '{project}'; names cannot be empty or contain '.', '/', or '\\'"
        )));
    }
    Ok(())
}

pub fn template(project: &str) -> String {
    format!(
        "name: {project}\nroot: ~/\n\n# pre_window: mise trust\n# startup_window: editor\n# startup_pane: 0\n# attach: true\n\nwindows:\n  - editor:\n      layout: main-vertical\n      panes:\n        - editor: $EDITOR\n        - shell:\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(root: &Path, body: &str) -> PathBuf {
        let path = root.join("project.yml");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_tmuxinator_core_and_preserves_command_order() {
        let temp = TempDir::new().unwrap();
        let yaml = format!(
            "name: app\nroot: {}\npre_window: prep\nstartup_window: dev\nwindows:\n  - dev:\n      layout: main-vertical\n      panes:\n        - editor: vim\n        - server: [cd api, cargo run]\n",
            temp.path().display()
        );
        let plan = ProjectPlan::from_file(&write_config(temp.path(), &yaml), None).unwrap();
        assert_eq!(plan.name, "app");
        assert_eq!(plan.tabs.len(), 1);
        let panes = collect_panes(&plan.tabs[0].layout);
        assert_eq!(panes[1].commands, ["prep", "cd api", "cargo run"]);
    }

    #[test]
    fn rejects_unknown_tmuxinator_fields() {
        let temp = TempDir::new().unwrap();
        let yaml = format!(
            "name: app\nroot: {}\non_project_start: echo nope\nwindows:\n  - shell:\n",
            temp.path().display()
        );
        assert!(ProjectPlan::from_file(&write_config(temp.path(), &yaml), None).is_err());
    }

    #[test]
    fn parses_native_tree() {
        let temp = TempDir::new().unwrap();
        let yaml = format!(
            "name: app\nroot: {}\ntabs:\n  - name: dev\n    layout:\n      split: right\n      ratio: 0.7\n      panes:\n        - name: editor\n          command: vim\n        - name: server\n          commands: [cd api, cargo run]\n",
            temp.path().display()
        );
        let plan = ProjectPlan::from_file(&write_config(temp.path(), &yaml), None).unwrap();
        assert_eq!(plan.tabs[0].layout.pane_count(), 2);
    }

    #[test]
    fn initializes_samples_without_overwriting_them() {
        let temp = TempDir::new().unwrap();
        let store = ConfigStore {
            global_dir: temp.path().join("herdinator"),
        };

        let created = store.initialize_samples().unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(
            fs::read_to_string(store.global_dir.join("native.yml")).unwrap(),
            SAMPLES[1].1
        );

        fs::write(store.global_dir.join("native.yml"), "custom").unwrap();
        assert!(store.initialize_samples().unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(store.global_dir.join("native.yml")).unwrap(),
            "custom"
        );
    }
}
