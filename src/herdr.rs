use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::layout::Direction;

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreatedWorkspace {
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreatedTab {
    pub tab_id: String,
    pub pane_id: String,
}

pub trait HerdrApi {
    fn ensure_server(&mut self) -> Result<()>;
    fn list_workspaces(&mut self) -> Result<Vec<Workspace>>;
    fn create_workspace(&mut self, label: &str, cwd: &Path) -> Result<CreatedWorkspace>;
    fn focus_workspace(&mut self, workspace_id: &str) -> Result<()>;
    fn close_workspace(&mut self, workspace_id: &str) -> Result<()>;
    fn rename_tab(&mut self, tab_id: &str, label: &str) -> Result<()>;
    fn create_tab(&mut self, workspace_id: &str, label: &str, cwd: &Path) -> Result<CreatedTab>;
    fn focus_tab(&mut self, tab_id: &str) -> Result<()>;
    fn split_pane(
        &mut self,
        pane_id: &str,
        direction: Direction,
        ratio: f64,
        cwd: &Path,
    ) -> Result<String>;
    fn rename_pane(&mut self, pane_id: &str, label: &str) -> Result<()>;
    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<()>;
    fn focus_pane(&mut self, pane_id: &str) -> Result<()>;
    fn attach(&mut self) -> Result<()>;
}

pub struct HerdrCli {
    binary: PathBuf,
}

impl Default for HerdrCli {
    fn default() -> Self {
        Self::new()
    }
}

impl HerdrCli {
    pub fn new() -> Self {
        Self {
            binary: std::env::var_os("HERDINATOR_HERDR_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("herdr")),
        }
    }

    pub fn version(&self) -> Result<String> {
        let output = self.raw([OsStr::new("--version")])?;
        ensure_success(output, "--version")
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn server_running(&self) -> bool {
        self.raw([OsStr::new("status"), OsStr::new("server")])
            .is_ok_and(|output| {
                output.status.success()
                    && status_is_running(&String::from_utf8_lossy(&output.stdout))
            })
    }

    fn json<I, S>(&self, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let description = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let output = ensure_success(self.raw(&args)?, &description)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn success<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let description = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        ensure_success(self.raw(&args)?, &description)?;
        Ok(())
    }

    fn raw<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|error| {
                Error::Herdr(format!(
                    "could not execute '{}': {error}",
                    self.binary.display()
                ))
            })
    }
}

impl HerdrApi for HerdrCli {
    fn ensure_server(&mut self) -> Result<()> {
        self.version()?;
        if self.server_running() {
            return Ok(());
        }

        Command::new(&self.binary)
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| Error::Herdr(format!("could not start server: {error}")))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.server_running() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(Error::Herdr(
            "server did not become ready within five seconds; run 'herdr server' for diagnostics"
                .into(),
        ))
    }

    fn list_workspaces(&mut self) -> Result<Vec<Workspace>> {
        let json = self.json(["workspace", "list"])?;
        let workspaces = result(&json)
            .get("workspaces")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Herdr("unexpected workspace list response".into()))?;
        let summaries = workspaces
            .iter()
            .map(|workspace| {
                Ok((
                    string_field(workspace, "workspace_id")?,
                    workspace
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        summaries
            .into_iter()
            .map(|(id, label)| {
                let panes = self.json(["pane", "list", "--workspace", &id])?;
                let cwd = result(&panes)
                    .get("panes")
                    .and_then(Value::as_array)
                    .and_then(|panes| panes.first())
                    .and_then(|pane| pane.get("cwd"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::Herdr(format!(
                            "workspace '{id}' has no pane cwd in its API response"
                        ))
                    })?;
                Ok(Workspace {
                    id,
                    label,
                    cwd: PathBuf::from(cwd),
                })
            })
            .collect()
    }

    fn create_workspace(&mut self, label: &str, cwd: &Path) -> Result<CreatedWorkspace> {
        let cwd = cwd.as_os_str();
        let json = self.json([
            OsStr::new("workspace"),
            OsStr::new("create"),
            OsStr::new("--cwd"),
            cwd,
            OsStr::new("--label"),
            OsStr::new(label),
            OsStr::new("--no-focus"),
        ])?;
        let result = result(&json);
        Ok(CreatedWorkspace {
            workspace_id: nested_string(result, &["workspace", "workspace_id"])?,
            tab_id: nested_string(result, &["tab", "tab_id"])?,
            pane_id: nested_string(result, &["root_pane", "pane_id"])?,
        })
    }

    fn focus_workspace(&mut self, workspace_id: &str) -> Result<()> {
        self.success(["workspace", "focus", workspace_id])
    }

    fn close_workspace(&mut self, workspace_id: &str) -> Result<()> {
        self.success(["workspace", "close", workspace_id])
    }

    fn rename_tab(&mut self, tab_id: &str, label: &str) -> Result<()> {
        self.success(["tab", "rename", tab_id, label])
    }

    fn create_tab(&mut self, workspace_id: &str, label: &str, cwd: &Path) -> Result<CreatedTab> {
        let json = self.json([
            OsStr::new("tab"),
            OsStr::new("create"),
            OsStr::new("--workspace"),
            OsStr::new(workspace_id),
            OsStr::new("--cwd"),
            cwd.as_os_str(),
            OsStr::new("--label"),
            OsStr::new(label),
            OsStr::new("--no-focus"),
        ])?;
        let result = result(&json);
        Ok(CreatedTab {
            tab_id: nested_string(result, &["tab", "tab_id"])?,
            pane_id: nested_string(result, &["root_pane", "pane_id"])?,
        })
    }

    fn focus_tab(&mut self, tab_id: &str) -> Result<()> {
        self.success(["tab", "focus", tab_id])
    }

    fn split_pane(
        &mut self,
        pane_id: &str,
        direction: Direction,
        ratio: f64,
        cwd: &Path,
    ) -> Result<String> {
        let ratio = ratio.to_string();
        let json = self.json([
            OsStr::new("pane"),
            OsStr::new("split"),
            OsStr::new(pane_id),
            OsStr::new("--direction"),
            OsStr::new(direction.as_str()),
            OsStr::new("--ratio"),
            OsStr::new(&ratio),
            OsStr::new("--cwd"),
            cwd.as_os_str(),
            OsStr::new("--no-focus"),
        ])?;
        nested_string(result(&json), &["pane", "pane_id"])
    }

    fn rename_pane(&mut self, pane_id: &str, label: &str) -> Result<()> {
        self.success(["pane", "rename", pane_id, label])
    }

    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<()> {
        self.success(["pane", "run", pane_id, command])
    }

    fn focus_pane(&mut self, pane_id: &str) -> Result<()> {
        // `zoom --off` targets and focuses a pane without changing the final zoom state.
        self.success(["pane", "zoom", pane_id, "--off"])
    }

    fn attach(&mut self) -> Result<()> {
        let status = Command::new(&self.binary)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|error| Error::Herdr(format!("could not attach: {error}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Herdr(format!(
                "attach exited with status {}",
                status.code().unwrap_or(-1)
            )))
        }
    }
}

fn ensure_success(output: Output, description: &str) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::Herdr(format!(
            "'{description}' failed{}{}",
            if stderr.is_empty() { "" } else { ": " },
            stderr
        )))
    }
}

fn result(value: &Value) -> &Value {
    value.get("result").unwrap_or(value)
}

fn string_field(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Herdr(format!("response is missing '{field}'")))
}

fn nested_string(value: &Value, path: &[&str]) -> Result<String> {
    let mut current = value;
    for field in path {
        current = current
            .get(field)
            .ok_or_else(|| Error::Herdr(format!("response is missing '{}'", path.join("."))))?;
    }
    current
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::Herdr(format!("'{}' is not a string", path.join("."))))
}

fn status_is_running(output: &str) -> bool {
    output.lines().any(|line| line.trim() == "status: running")
}

#[cfg(test)]
mod tests {
    use super::status_is_running;

    #[test]
    fn distinguishes_running_and_stopped_status() {
        assert!(status_is_running("status: running\nversion: 0.9.1\n"));
        assert!(!status_is_running(
            "status: not running\nsocket: /tmp/herdr.sock\n"
        ));
    }
}
