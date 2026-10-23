use std::path::Path;

use crate::config::{ProjectPlan, Selector};
use crate::error::{Error, Result};
use crate::herdr::{CreatedTab, HerdrApi, Workspace};
use crate::layout::{LayoutNode, PanePlan};

#[derive(Debug, PartialEq)]
pub enum StartOutcome {
    Created,
    Existing,
}

pub struct ProjectManager<A> {
    api: A,
}

impl<A: HerdrApi> ProjectManager<A> {
    pub fn new(api: A) -> Self {
        Self { api }
    }

    pub fn start(&mut self, plan: &ProjectPlan, attach: bool) -> Result<StartOutcome> {
        self.launch(plan, attach, false)
    }

    /// Closes a matching workspace and rebuilds it, so configuration edits take
    /// effect. Panes and their processes are lost, exactly like `stop` + `start`.
    pub fn restart(&mut self, plan: &ProjectPlan, attach: bool) -> Result<StartOutcome> {
        self.launch(plan, attach, true)
    }

    fn launch(&mut self, plan: &ProjectPlan, attach: bool, restart: bool) -> Result<StartOutcome> {
        self.api.ensure_server()?;
        let workspaces = self.api.list_workspaces()?;
        if let Some(workspace) = find_existing_for_start(&workspaces, plan)? {
            if !restart {
                self.api.focus_workspace(&workspace.id)?;
                if attach {
                    self.api.attach()?;
                }
                return Ok(StartOutcome::Existing);
            }
            self.api.close_workspace(&workspace.id)?;
        }

        let created = self.api.create_workspace(&plan.name, &plan.tabs[0].root)?;
        let result = self.build_project(
            plan,
            &created.workspace_id,
            &created.tab_id,
            &created.pane_id,
        );
        if let Err(error) = result {
            if let Err(rollback) = self.api.close_workspace(&created.workspace_id) {
                // Best-effort cleanup: keep the original failure as the error,
                // but never fail silently and leave a half-built workspace behind.
                eprintln!(
                    "warning: could not close workspace '{}' after failure: {rollback}",
                    created.workspace_id
                );
            }
            return Err(error);
        }

        if attach {
            self.api.attach()?;
        }
        Ok(StartOutcome::Created)
    }

    pub fn stop(&mut self, plan: &ProjectPlan) -> Result<()> {
        self.api.ensure_server()?;
        let workspaces = self.api.list_workspaces()?;
        let workspace = find_existing_for_stop(&workspaces, plan)?.ok_or_else(|| {
            Error::WorkspaceConflict(format!("workspace '{}' is not running", plan.name))
        })?;
        self.api.close_workspace(&workspace.id)
    }

    fn build_project(
        &mut self,
        plan: &ProjectPlan,
        workspace_id: &str,
        first_tab_id: &str,
        first_pane_id: &str,
    ) -> Result<()> {
        let mut runtime_tabs = Vec::new();
        for (index, tab) in plan.tabs.iter().enumerate() {
            let created = if index == 0 {
                self.api.rename_tab(first_tab_id, &tab.name)?;
                CreatedTab {
                    tab_id: first_tab_id.to_string(),
                    pane_id: first_pane_id.to_string(),
                }
            } else {
                self.api.create_tab(workspace_id, &tab.name, &tab.root)?
            };
            let mut panes = Vec::new();
            build_layout(
                &mut self.api,
                &tab.layout,
                &created.pane_id,
                &tab.root,
                &mut panes,
            )?;
            configure_panes(&mut self.api, &panes)?;

            if let Some(selector) = tab.focused_pane.as_ref() {
                let pane_id = resolve_pane(selector, &panes, &tab.name)?;
                self.api.focus_pane(pane_id)?;
            }
            runtime_tabs.push(RuntimeTab {
                id: created.tab_id,
                name: tab.name.clone(),
                panes,
            });
        }

        let startup_tab_index = resolve_tab(plan.startup_window.as_ref(), &runtime_tabs)?;
        let startup_tab = &runtime_tabs[startup_tab_index];
        let pane_selector = plan
            .startup_pane
            .as_ref()
            .or(plan.tabs[startup_tab_index].focused_pane.as_ref());
        let startup_pane = match pane_selector {
            Some(selector) => resolve_pane(selector, &startup_tab.panes, &startup_tab.name)?,
            None => &startup_tab.panes[0].id,
        };

        self.api.focus_workspace(workspace_id)?;
        self.api.focus_tab(&startup_tab.id)?;
        self.api.focus_pane(startup_pane)?;
        Ok(())
    }
}

struct RuntimeTab {
    id: String,
    name: String,
    panes: Vec<RuntimePane>,
}

struct RuntimePane {
    id: String,
    plan: PanePlan,
}

fn find_existing_for_start<'a>(
    workspaces: &'a [Workspace],
    plan: &ProjectPlan,
) -> Result<Option<&'a Workspace>> {
    let named: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.label == plan.name)
        .collect();
    if named.is_empty() {
        return Ok(None);
    }
    let matching: Vec<_> = named
        .iter()
        .copied()
        .filter(|workspace| {
            workspace
                .cwd
                .as_deref()
                .is_some_and(|cwd| same_path(cwd, &plan.tabs[0].root))
        })
        .collect();
    match matching.as_slice() {
        [workspace] if named.len() == 1 => Ok(Some(*workspace)),
        // A same-named workspace without pane information cannot be verified.
        // Refuse instead of creating a duplicate that would later be ambiguous.
        [] if named.iter().any(|workspace| workspace.cwd.is_none()) => {
            Err(Error::WorkspaceConflict(format!(
                "workspace '{}' exists but Herdr reported no pane for it; close it with 'herdinator stop {}'",
                plan.name, plan.name
            )))
        }
        [] => Err(Error::WorkspaceConflict(format!(
            "workspace '{}' already exists with a different root",
            plan.name
        ))),
        _ => Err(Error::WorkspaceConflict(format!(
            "multiple workspaces match '{}' and {}",
            plan.name,
            plan.tabs[0].root.display()
        ))),
    }
}

fn find_existing_for_stop<'a>(
    workspaces: &'a [Workspace],
    plan: &ProjectPlan,
) -> Result<Option<&'a Workspace>> {
    let named: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.label == plan.name)
        .collect();
    match named.as_slice() {
        [] => Ok(None),
        [workspace] => Ok(Some(*workspace)),
        _ => Err(Error::WorkspaceConflict(format!(
            "multiple workspaces are named '{}'; close one of them manually",
            plan.name
        ))),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

fn build_layout<A: HerdrApi>(
    api: &mut A,
    node: &LayoutNode,
    pane_id: &str,
    cwd: &Path,
    panes: &mut Vec<RuntimePane>,
) -> Result<()> {
    match node {
        LayoutNode::Pane { pane } => panes.push(RuntimePane {
            id: pane_id.to_string(),
            plan: pane.clone(),
        }),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let second_id = api.split_pane(pane_id, *direction, *ratio, cwd)?;
            build_layout(api, first, pane_id, cwd, panes)?;
            build_layout(api, second, &second_id, cwd, panes)?;
        }
    }
    Ok(())
}

fn configure_panes<A: HerdrApi>(api: &mut A, panes: &[RuntimePane]) -> Result<()> {
    for pane in panes {
        if let Some(name) = pane.plan.name.as_deref() {
            api.rename_pane(&pane.id, name)?;
        }
        for command in &pane.plan.commands {
            api.run_in_pane(&pane.id, command)?;
        }
    }
    Ok(())
}

fn resolve_tab(selector: Option<&Selector>, tabs: &[RuntimeTab]) -> Result<usize> {
    match selector {
        None => Ok(0),
        Some(Selector::Index(index)) if *index < tabs.len() => Ok(*index),
        Some(Selector::Name(name)) => tabs
            .iter()
            .position(|tab| &tab.name == name)
            .ok_or_else(|| Error::InvalidConfig(format!("startup_window '{name}' was not found"))),
        Some(Selector::Index(index)) => Err(Error::InvalidConfig(format!(
            "startup_window index {index} is out of range"
        ))),
    }
}

fn resolve_pane<'a>(selector: &Selector, panes: &'a [RuntimePane], tab: &str) -> Result<&'a str> {
    let pane = match selector {
        Selector::Index(index) => panes.get(*index),
        Selector::Name(name) => panes
            .iter()
            .find(|pane| pane.plan.name.as_deref() == Some(name)),
    };
    pane.map(|pane| pane.id.as_str())
        .ok_or_else(|| Error::InvalidConfig(format!("pane selector in tab '{tab}' was not found")))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use super::*;
    use crate::config::TabPlan;
    use crate::herdr::{CreatedWorkspace, HerdrApi};
    use crate::layout::Direction;

    #[derive(Default)]
    struct FakeApi {
        workspaces: Vec<Workspace>,
        events: Vec<String>,
        panes: VecDeque<String>,
        fail_run: bool,
        fail_close: bool,
    }

    impl HerdrApi for FakeApi {
        fn ensure_server(&mut self) -> Result<()> {
            self.events.push("server".into());
            Ok(())
        }
        fn list_workspaces(&mut self) -> Result<Vec<Workspace>> {
            Ok(self.workspaces.clone())
        }
        fn create_workspace(&mut self, label: &str, _: &Path) -> Result<CreatedWorkspace> {
            self.events.push(format!("create:{label}"));
            Ok(CreatedWorkspace {
                workspace_id: "w1".into(),
                tab_id: "t1".into(),
                pane_id: "p1".into(),
            })
        }
        fn focus_workspace(&mut self, id: &str) -> Result<()> {
            self.events.push(format!("focus-workspace:{id}"));
            Ok(())
        }
        fn close_workspace(&mut self, id: &str) -> Result<()> {
            self.events.push(format!("close:{id}"));
            if self.fail_close {
                Err(Error::Herdr("close failed".into()))
            } else {
                Ok(())
            }
        }
        fn rename_tab(&mut self, _: &str, label: &str) -> Result<()> {
            self.events.push(format!("tab:{label}"));
            Ok(())
        }
        fn create_tab(&mut self, _: &str, label: &str, _: &Path) -> Result<CreatedTab> {
            Ok(CreatedTab {
                tab_id: format!("t-{label}"),
                pane_id: format!("p-{label}"),
            })
        }
        fn focus_tab(&mut self, id: &str) -> Result<()> {
            self.events.push(format!("focus-tab:{id}"));
            Ok(())
        }
        fn split_pane(&mut self, _: &str, _: Direction, _: f64, _: &Path) -> Result<String> {
            Ok(self.panes.pop_front().unwrap_or_else(|| "p2".into()))
        }
        fn rename_pane(&mut self, _: &str, label: &str) -> Result<()> {
            self.events.push(format!("rename:{label}"));
            Ok(())
        }
        fn run_in_pane(&mut self, _: &str, command: &str) -> Result<()> {
            self.events.push(format!("run:{command}"));
            if self.fail_run {
                Err(Error::Herdr("run failed".into()))
            } else {
                Ok(())
            }
        }
        fn focus_pane(&mut self, id: &str) -> Result<()> {
            self.events.push(format!("focus-pane:{id}"));
            Ok(())
        }
        fn attach(&mut self) -> Result<()> {
            self.events.push("attach".into());
            Ok(())
        }
    }

    fn plan(root: PathBuf) -> ProjectPlan {
        ProjectPlan {
            name: "app".into(),
            root: root.clone(),
            attach: true,
            startup_window: None,
            startup_pane: None,
            tabs: vec![TabPlan {
                name: "dev".into(),
                root,
                focused_pane: None,
                layout: LayoutNode::pane(PanePlan {
                    name: Some("shell".into()),
                    commands: vec!["echo ok".into()],
                }),
            }],
        }
    }

    #[test]
    fn existing_workspace_is_only_focused() {
        let root = std::env::temp_dir();
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "existing".into(),
                label: "app".into(),
                cwd: Some(root.clone()),
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        assert_eq!(
            manager.start(&plan(root), false).unwrap(),
            StartOutcome::Existing
        );
        assert_eq!(manager.api.events, ["server", "focus-workspace:existing"]);
    }

    #[test]
    fn failure_rolls_back_new_workspace() {
        let api = FakeApi {
            fail_run: true,
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        assert!(manager.start(&plan(std::env::temp_dir()), false).is_err());
        assert!(manager.api.events.contains(&"close:w1".to_string()));
    }

    #[test]
    fn rollback_failure_keeps_the_original_error() {
        let api = FakeApi {
            fail_run: true,
            fail_close: true,
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        let error = manager
            .start(&plan(std::env::temp_dir()), false)
            .unwrap_err();
        assert!(matches!(&error, Error::Herdr(message) if message == "run failed"));
        assert!(manager.api.events.contains(&"close:w1".to_string()));
    }

    #[test]
    fn workspace_without_cwd_does_not_block_unrelated_start() {
        let root = std::env::temp_dir();
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "empty".into(),
                label: "other".into(),
                cwd: None,
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        assert_eq!(
            manager.start(&plan(root), false).unwrap(),
            StartOutcome::Created
        );
        assert!(manager.api.events.contains(&"create:app".to_string()));
    }

    #[test]
    fn start_refuses_unverifiable_matching_workspace() {
        let root = std::env::temp_dir();
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "empty".into(),
                label: "app".into(),
                cwd: None,
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        let error = manager.start(&plan(root), false).unwrap_err();
        let Error::WorkspaceConflict(message) = error else {
            panic!("expected WorkspaceConflict, got {error:?}");
        };
        assert!(message.contains("herdinator stop app"), "{message}");
        assert!(!manager
            .api
            .events
            .iter()
            .any(|event| event.starts_with("create:")));
    }

    #[test]
    fn restart_closes_and_recreates_the_matching_workspace() {
        let root = std::env::temp_dir();
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "existing".into(),
                label: "app".into(),
                cwd: Some(root.clone()),
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        assert_eq!(
            manager.restart(&plan(root), false).unwrap(),
            StartOutcome::Created
        );
        let events = &manager.api.events;
        let close = events.iter().position(|event| event == "close:existing");
        let create = events.iter().position(|event| event == "create:app");
        assert!(close.is_some() && create.is_some(), "{events:?}");
        assert!(close < create, "{events:?}");
        assert!(
            !events
                .iter()
                .any(|event| event == "focus-workspace:existing"),
            "{events:?}"
        );
    }

    #[test]
    fn restart_without_an_existing_workspace_creates_one() {
        let root = std::env::temp_dir();
        let mut manager = ProjectManager::new(FakeApi::default());
        assert_eq!(
            manager.restart(&plan(root), false).unwrap(),
            StartOutcome::Created
        );
        assert!(manager.api.events.contains(&"create:app".to_string()));
    }

    #[test]
    fn stop_closes_workspace_without_cwd() {
        let root = std::env::temp_dir();
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "empty".into(),
                label: "app".into(),
                cwd: None,
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        manager.stop(&plan(root)).unwrap();
        assert_eq!(manager.api.events, ["server", "close:empty"]);
    }

    #[test]
    fn stop_closes_workspace_even_when_root_differs() {
        let configured = std::env::temp_dir().join("herdinator-configured");
        let other = std::env::temp_dir().join("herdinator-other");
        let api = FakeApi {
            workspaces: vec![Workspace {
                id: "w9".into(),
                label: "app".into(),
                cwd: Some(other),
            }],
            ..Default::default()
        };
        let mut manager = ProjectManager::new(api);
        manager.stop(&plan(configured)).unwrap();
        assert_eq!(manager.api.events, ["server", "close:w9"]);
    }
}
