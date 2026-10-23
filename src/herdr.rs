use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use wait_timeout::ChildExt;

use crate::error::{Error, Result};
use crate::layout::Direction;

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    /// First pane's working directory, when Herdr can report one.
    ///
    /// `None` means the workspace exposed no usable pane information (no
    /// panes, or a failed `pane list`). Such workspaces are kept visible so
    /// callers can still match them by name instead of creating duplicates.
    pub cwd: Option<PathBuf>,
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

/// Upper bound for non-interactive Herdr commands. `pane run` is expected to
/// return immediately, so anything slower means the server is unhealthy.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Where Herdr API commands are executed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HerdrTarget {
    /// The Herdr server on this machine.
    #[default]
    Local,
    /// A saved Herdr SSH machine, addressed by label or id.
    Machine(String),
}

impl HerdrTarget {
    /// Arguments Herdr needs before the subcommand to reach this target.
    fn prefix(&self) -> Vec<OsString> {
        match self {
            Self::Local => Vec::new(),
            Self::Machine(machine) => {
                vec![OsString::from("--machine"), OsString::from(machine)]
            }
        }
    }
}

pub struct HerdrCli {
    binary: PathBuf,
    verbose: bool,
    timeout: Duration,
    target: HerdrTarget,
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
            verbose: false,
            timeout: COMMAND_TIMEOUT,
            target: HerdrTarget::Local,
        }
    }

    /// Run Herdr API commands against `target` instead of the local server.
    pub fn target(mut self, target: HerdrTarget) -> Self {
        self.target = target;
        self
    }

    /// Trace every Herdr command on stderr.
    pub fn verbose(mut self, enabled: bool) -> Self {
        self.verbose = enabled;
        self
    }

    pub fn version(&self) -> Result<String> {
        let output = self.raw([OsStr::new("--version")])?;
        ensure_success(output, "--version")
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn server_running(&self) -> bool {
        self.api([OsStr::new("status"), OsStr::new("server")])
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
        let args = collect_args(args);
        let description = describe(&args);
        let output = ensure_success(self.api(&args)?, &description)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn success<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_args(args);
        let description = describe(&args);
        ensure_success(self.api(&args)?, &description)?;
        Ok(())
    }

    /// Command line executed for `args`, including a target selector when the
    /// command has to travel to a saved machine.
    fn command_line(&self, args: Vec<OsString>, targeted: bool) -> Vec<OsString> {
        if !targeted {
            return args;
        }
        let mut prefixed = self.target.prefix();
        prefixed.extend(args);
        prefixed
    }

    fn raw<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(args, false)
    }

    /// Runs an API command, routing it through `--machine` when needed.
    fn api<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(args, true)
    }

    fn run<I, S>(&self, args: I, targeted: bool) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = self.command_line(collect_args(args), targeted);
        let description = describe(&args);
        if self.verbose {
            eprintln!("herdr {description}");
        }

        let mut child = Command::new(&self.binary)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Error::Herdr(format!(
                    "could not execute '{}': {error}",
                    self.binary.display()
                ))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Herdr("could not capture herdr stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Herdr("could not capture herdr stderr".into()))?;
        // Drain both pipes concurrently so a chatty command cannot block on a
        // full pipe buffer while we are waiting for it to exit.
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));

        let status = match child.wait_timeout(self.timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                terminate(&mut child, stdout_reader, stderr_reader);
                return Err(Error::Timeout {
                    command: format!("herdr {description}"),
                    seconds: self.timeout.as_secs().max(1),
                });
            }
            Err(error) => {
                terminate(&mut child, stdout_reader, stderr_reader);
                return Err(Error::Herdr(format!(
                    "could not wait for 'herdr {description}': {error}"
                )));
            }
        };

        Ok(Output {
            status,
            stdout: stdout_reader.join().unwrap_or_default(),
            stderr: stderr_reader.join().unwrap_or_default(),
        })
    }

    /// First pane cwd for every workspace, gathered with a single `pane list`
    /// call instead of one call per workspace.
    ///
    /// Returns `None` when the global listing is unavailable, so callers can
    /// fall back to querying each workspace individually.
    fn pane_cwds(&self) -> Option<HashMap<String, PathBuf>> {
        let json = self.json(["pane", "list"]).ok()?;
        let panes = result(&json).get("panes")?.as_array()?;
        let mut cwds = HashMap::new();
        for pane in panes {
            let Some(workspace_id) = pane.get("workspace_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(cwd) = pane.get("cwd").and_then(Value::as_str) else {
                continue;
            };
            cwds.entry(workspace_id.to_string())
                .or_insert_with(|| PathBuf::from(cwd));
        }
        Some(cwds)
    }

    /// Best-effort lookup of a workspace's first pane cwd.
    ///
    /// A workspace without panes, or one whose `pane list` call fails, must not
    /// take down the whole workspace listing.
    fn pane_cwd(&self, workspace_id: &str) -> Option<PathBuf> {
        let json = self
            .json(["pane", "list", "--workspace", workspace_id])
            .ok()?;
        let panes = result(&json).get("panes")?.as_array()?;
        panes.first()?.get("cwd")?.as_str().map(PathBuf::from)
    }
}

impl HerdrApi for HerdrCli {
    fn ensure_server(&mut self) -> Result<()> {
        self.version()?;
        if self.server_running() {
            return Ok(());
        }
        if self.target != HerdrTarget::Local {
            return Err(Error::Herdr(
                "the Herdr server on a remote machine must be started manually".into(),
            ));
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

        let cwds = self.pane_cwds();
        Ok(summaries
            .into_iter()
            .map(|(id, label)| {
                let cwd = match &cwds {
                    Some(cwds) => cwds.get(&id).cloned(),
                    None => self.pane_cwd(&id),
                };
                Workspace { id, label, cwd }
            })
            .collect())
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
        // WORKAROUND: Herdr has no "focus this pane id" command. `pane focus`
        // only moves focus to a *neighbouring* pane (`--direction left|right|
        // up|down`), so it cannot target an arbitrary pane.
        //
        // `pane zoom <id> --off` is the only command that addresses a pane by id
        // and focuses it. Zoom is set to off unconditionally, so this is
        // idempotent: calling it for the already focused pane is a no-op and it
        // never leaves a pane zoomed.
        //
        // Replace this with `herdr pane focus <pane_id>` if Herdr ever adds it.
        self.success(["pane", "zoom", pane_id, "--off"])
    }

    fn attach(&mut self) -> Result<()> {
        if self.verbose {
            eprintln!("herdr attach (interactive, no timeout)");
        }
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

fn collect_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect()
}

fn describe(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}

fn terminate(
    child: &mut std::process::Child,
    stdout: thread::JoinHandle<Vec<u8>>,
    stderr: thread::JoinHandle<Vec<u8>>,
) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout.join();
    let _ = stderr.join();
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::{status_is_running, HerdrApi, HerdrCli, HerdrTarget};
    use crate::error::Error;

    fn shell(timeout: Duration) -> HerdrCli {
        HerdrCli {
            binary: PathBuf::from("/bin/sh"),
            verbose: false,
            timeout,
            target: HerdrTarget::Local,
        }
    }

    /// Builds a HerdrCli backed by the shared fake-herdr fixture.
    ///
    /// The fixture is a checked-in file, so `exec` never touches bytes written
    /// at runtime: writing an executable and immediately running it races with
    /// concurrent forks and fails intermittently with ETXTBSY.
    fn script_cli(mode: &str) -> (TempDir, HerdrCli) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fake-herdr");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-herdr.sh");
        std::os::unix::fs::symlink(&fixture, &path).unwrap();
        fs::write(dir.path().join("mode"), mode).unwrap();
        let cli = HerdrCli {
            binary: path,
            verbose: false,
            timeout: Duration::from_secs(5),
            target: HerdrTarget::Local,
        };
        (dir, cli)
    }

    #[test]
    fn distinguishes_running_and_stopped_status() {
        assert!(status_is_running("status: running\nversion: 0.9.1\n"));
        assert!(!status_is_running(
            "status: not running\nsocket: /tmp/herdr.sock\n"
        ));
    }

    #[test]
    fn captures_output_of_fast_commands() {
        let output = shell(Duration::from_secs(5))
            .raw(["-c", "printf hello"])
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello");
    }

    #[test]
    fn drains_large_output_without_deadlocking() {
        let output = shell(Duration::from_secs(5))
            .raw(["-c", "yes x | head -c 200000"])
            .unwrap();
        assert_eq!(output.stdout.len(), 200_000);
    }

    #[test]
    fn list_workspaces_uses_a_single_pane_listing() {
        let (_dir, mut cli) = script_cli("grouping");
        let workspaces = cli.list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 3);
        assert_eq!(workspaces[0].cwd, Some(PathBuf::from("/first")));
        assert_eq!(workspaces[1].cwd, Some(PathBuf::from("/other")));
        assert_eq!(workspaces[2].cwd, None);
    }

    #[test]
    fn list_workspaces_falls_back_to_per_workspace_listing() {
        let (_dir, mut cli) = script_cli("fallback");
        let workspaces = cli.list_workspaces().unwrap();
        assert_eq!(workspaces[0].cwd, Some(PathBuf::from("/fallback")));
    }

    #[test]
    fn list_workspaces_survives_a_failing_workspace_query() {
        let (_dir, mut cli) = script_cli("failing");
        let workspaces = cli.list_workspaces().unwrap();
        assert_eq!(workspaces[0].cwd, None);
    }

    #[test]
    fn machine_target_prefixes_api_commands() {
        let (_dir, cli) = script_cli("machine");
        let cli = cli.target(HerdrTarget::Machine("box".into()));
        assert_eq!(
            cli.json(["pane", "list"]).unwrap()["result"]["panes"],
            serde_json::json!([])
        );
    }

    #[test]
    fn version_checks_stay_local_for_machine_targets() {
        let (_dir, cli) = script_cli("machine");
        let cli = cli.target(HerdrTarget::Machine("box".into()));
        assert_eq!(cli.version().unwrap(), "herdr 0.9.1");
    }

    #[test]
    fn ensure_server_refuses_to_start_a_remote_server() {
        let (_dir, mut cli) = script_cli("remote-down");
        cli.target = HerdrTarget::Machine("box".into());
        let error = cli.ensure_server().unwrap_err();
        assert!(
            error.to_string().contains("must be started manually"),
            "{error}"
        );
    }

    #[test]
    fn kills_commands_that_exceed_the_timeout() {
        let started = Instant::now();
        let error = shell(Duration::from_millis(200))
            .raw(["-c", "sleep 30"])
            .unwrap_err();
        assert!(
            matches!(&error, Error::Timeout { .. }),
            "expected timeout, got {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out command was not killed promptly"
        );
    }
}
