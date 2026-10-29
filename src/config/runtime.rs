//! 明示Ruby拡張だけのrun共有状態。通常のnative設定/getterは子Rubyを作らない。

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use serde_yaml_ng::Value;

const SCRIPT: &str = include_str!("../../lib/sonicop/runtime.rb");
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct Snapshot {
    pub(super) names: Vec<String>,
    pub(super) departments: Vec<String>,
    pub(super) loaded_features: Vec<String>,
    pub(super) loaded_plugins: Vec<String>,
    #[serde(default)]
    defaults: Vec<DefaultChange>,
}

#[derive(Clone, Debug, Deserialize)]
struct DefaultChange {
    path: Vec<String>,
    #[serde(default)]
    remove: bool,
    yaml: Option<String>,
}

impl Snapshot {
    pub(super) fn plugin_loaded(&self, name: &str) -> bool {
        self.loaded_features
            .iter()
            .chain(&self.loaded_plugins)
            .any(|feature| feature == name)
    }

    pub(super) fn apply_defaults(&self, value: &mut Value) -> Result<()> {
        for change in &self.defaults {
            let mut current = &mut *value;
            let Some((last, parents)) = change.path.split_last() else {
                if !change.remove {
                    *current = parse_yaml(
                        change
                            .yaml
                            .as_deref()
                            .context("missing Ruby default value")?,
                    )?;
                }
                continue;
            };
            for parent in parents {
                if !current.is_mapping() {
                    *current = Value::Mapping(Default::default());
                }
                current = current
                    .as_mapping_mut()
                    .unwrap()
                    .entry(Value::String(parent.clone()))
                    .or_insert(Value::Null);
            }
            if !current.is_mapping() {
                *current = Value::Mapping(Default::default());
            }
            let mapping = current.as_mapping_mut().unwrap();
            if change.remove {
                mapping.remove(Value::String(last.clone()));
            } else {
                mapping.insert(
                    Value::String(last.clone()),
                    parse_yaml(
                        change
                            .yaml
                            .as_deref()
                            .context("missing Ruby default value")?,
                    )?,
                );
            }
        }
        Ok(())
    }
}

fn parse_yaml(text: &str) -> Result<Value> {
    serde_yaml_ng::from_str(&super::yaml_booleans::normalize(text))
        .context("Ruby extension returned unsupported configuration data")
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RuntimeError {
    #[serde(rename = "class")]
    pub(crate) class: String,
    message: String,
    kind: String,
    status: Option<i32>,
    #[serde(skip)]
    color: bool,
}

impl RuntimeError {
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
    pub(crate) fn kind(&self) -> &str {
        &self.kind
    }
    pub(crate) fn color(&self) -> bool {
        self.color
    }
    pub(crate) fn status(&self) -> Option<i32> {
        self.status
    }
    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(runtime) = error.downcast_mut::<Self>() {
            runtime.color = color;
        }
        error
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&self.message)
    }
}
impl std::error::Error for RuntimeError {}

fn decode_error(value: Json) -> Result<RuntimeError> {
    let error: RuntimeError = serde_json::from_value(value)?;
    if error.class.is_empty()
        || !matches!(
            error.kind.as_str(),
            "plain" | "rubocop" | "argument" | "ambiguous" | "exit" | "interrupt"
        )
        || (matches!(error.kind.as_str(), "exit" | "interrupt") && error.status.is_none())
    {
        bail!("invalid Ruby extension exception descriptor");
    }
    Ok(error)
}

#[derive(Debug, Default)]
struct State {
    bridge: Option<Bridge>,
    snapshot: Arc<Snapshot>,
    redirect: bool,
    color: bool,
    terminal: Option<RuntimeError>,
    seed_unqualified: bool,
    seed_default: bool,
    load_path: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeSession {
    state: Mutex<State>,
}

impl RuntimeSession {
    pub(crate) fn from_wrapper() -> Result<Self> {
        let session = Self::default();
        let Some(path) = std::env::var_os("SONICOP_RUNTIME_HANDOFF") else {
            return Ok(session);
        };
        let path = PathBuf::from(path);
        let token = std::env::var("SONICOP_RUNTIME_HANDOFF_TOKEN")
            .context("missing Ruby wrapper handoff token")?;
        let parent = path.parent().context("invalid Ruby wrapper handoff path")?;
        let valid = token.len() == 64
            && token.bytes().all(|byte| byte.is_ascii_hexdigit())
            && path
                .file_name()
                .is_some_and(|name| name == "load-path.json")
            && parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("sonicop-ruby-handoff-"))
            && fs::symlink_metadata(parent)?.is_dir()
            && fs::symlink_metadata(&path)?.is_file();
        anyhow::ensure!(valid, "invalid Ruby wrapper handoff ownership");
        let bytes = fs::read(&path)?;
        let value: Json = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            value.get("token").and_then(Json::as_str) == Some(token.as_str()),
            "invalid Ruby wrapper handoff token"
        );
        anyhow::ensure!(
            value.get("format").and_then(Json::as_str) == Some("sonicop-ruby-context")
                && value.get("version").and_then(Json::as_u64) == Some(1),
            "invalid Ruby wrapper handoff format"
        );
        let paths: Vec<String> = serde_json::from_value(
            value
                .get("paths")
                .cloned()
                .context("missing Ruby wrapper load path")?,
        )?;
        // 内容を確認した一つの生成物だけを回収し、未知entryを再帰削除しない。
        if fs::symlink_metadata(parent)?.is_dir()
            && fs::symlink_metadata(&path)?.is_file()
            && fs::read(&path)? == bytes
        {
            fs::remove_file(&path)?;
            let _ = fs::remove_dir(parent);
        }
        session.state.lock().unwrap().load_path = Some(paths);
        Ok(session)
    }

    pub(super) fn observe_default_query(&self) -> Result<()> {
        {
            let mut state = self.state.lock().expect("Ruby runtime state poisoned");
            if state.bridge.is_none() {
                state.seed_default = true;
                return Ok(());
            }
        }
        // ConfigValidator#validateのkey partitionで自然に到達するgetterをVMでも実行する。
        self.request(json!({"action":"default"}))?;
        Ok(())
    }

    pub(super) fn note_unqualified_query(&self) {
        let mut state = self.state.lock().expect("Ruby runtime state poisoned");
        if state.bridge.is_none() {
            state.seed_unqualified = true;
        }
    }

    pub(crate) fn finish(&self, status: i32) -> Result<i32> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Ruby runtime state poisoned"))?;
        state
            .bridge
            .as_mut()
            .map_or(Ok(status), |bridge| bridge.finish(status))
    }

    pub(crate) fn active(&self) -> bool {
        self.state
            .lock()
            .expect("Ruby runtime state poisoned")
            .bridge
            .is_some()
    }

    pub(super) fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(
            &self
                .state
                .lock()
                .expect("Ruby runtime state poisoned")
                .snapshot,
        )
    }

    pub(crate) fn phase(&self, redirect: bool, color: bool) {
        let mut state = self.state.lock().expect("Ruby runtime state poisoned");
        state.redirect = redirect;
        state.color = color;
    }

    pub(crate) fn read_stdin(&self) -> Result<Vec<u8>> {
        let response = self.request(json!({"action":"stdin"}))?;
        serde_json::from_value(
            response
                .get("bytes")
                .cloned()
                .context("missing Ruby stdin payload")?,
        )
        .context("invalid Ruby stdin payload")
    }

    pub(crate) fn cli_extensions(&self, entries: &[(bool, String)]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.request(json!({"action":"extensions", "cli":true, "entries": entries.iter().map(|(plugin,name)| json!({"plugin":plugin,"name":name})).collect::<Vec<_>>() }))?;
        Ok(())
    }

    pub(super) fn has_extensions(value: &Value) -> bool {
        value.as_mapping().is_some_and(|mapping| {
            ["require", "plugins"].iter().any(|key| {
                mapping.get(*key).is_some_and(|value| {
                    !matches!(value, Value::Null)
                        && value
                            .as_sequence()
                            .is_none_or(|entries| !entries.is_empty())
                })
            })
        })
    }

    pub(super) fn config_extensions(&self, value: &Value, origin: &str) -> Result<()> {
        if !Self::has_extensions(value) {
            return Ok(());
        }
        self.request(json!({"action":"extensions", "cli":false, "yaml":serde_yaml_ng::to_string(value)?, "origin":origin}))?;
        Ok(())
    }

    pub(super) fn namespace(&self, names: &[String], origin: &str) -> Result<Namespace> {
        let response =
            self.request_raw(json!({"action":"namespace", "names":names, "origin":origin}))?;
        Ok(Namespace {
            qualified: serde_json::from_value(
                response
                    .get("qualified")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )?,
            warnings: serde_json::from_value(
                response
                    .get("warnings")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )?,
            error: response
                .get("error")
                .cloned()
                .map(decode_error)
                .transpose()?,
        })
    }

    fn request(&self, request: Json) -> Result<Json> {
        let response = self.request_raw(request)?;
        if let Some(error) = response.get("error") {
            return Err(decode_error(error.clone())?.into());
        }
        Ok(response)
    }

    fn request_raw(&self, mut request: Json) -> Result<Json> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Ruby runtime state poisoned"))?;
        if let Some(terminal) = &state.terminal {
            return Err(terminal.clone().into());
        }
        if state.bridge.is_none() {
            state.bridge = Some(Bridge::spawn(
                state.seed_unqualified,
                state.seed_default,
                state.load_path.as_deref(),
            )?);
        }
        request["redirect"] = json!(state.redirect);
        request["color"] = json!(state.color);
        let bridge = state.bridge.as_mut().unwrap();
        let boot = bridge.boot()?;
        if let Some(error) = boot.get("error") {
            return Err(decode_error(error.clone())?.into());
        }
        let response = bridge.request(&request)?;
        if let Some(snapshot) = response.get("snapshot") {
            state.snapshot = Arc::new(serde_json::from_value(snapshot.clone())?);
        }
        if let Some(error) = response.get("error") {
            let error = decode_error(error.clone())?;
            if matches!(error.kind.as_str(), "exit" | "interrupt") {
                state.terminal = Some(error);
            }
        }
        Ok(response)
    }
}

pub(super) struct Namespace {
    pub(super) qualified: Vec<String>,
    pub(super) warnings: Vec<String>,
    pub(super) error: Option<RuntimeError>,
}

#[derive(Debug)]
struct Bridge {
    child: Child,
    directory: PathBuf,
    owned: HashSet<PathBuf>,
    sequence: u64,
    boot: Option<Json>,
    finished: Option<i32>,
}

impl Bridge {
    fn spawn(
        seed_unqualified: bool,
        seed_default: bool,
        load_path: Option<&[String]>,
    ) -> Result<Self> {
        let ruby = std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into());
        Self::spawn_script(
            &ruby,
            SCRIPT,
            seed_unqualified,
            seed_default,
            false,
            load_path,
        )
    }

    fn spawn_script(
        ruby: &std::ffi::OsStr,
        script: &str,
        seed_unqualified: bool,
        seed_default: bool,
        clean: bool,
        load_path: Option<&[String]>,
    ) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("sonicop-runtime-")
            .tempdir()?
            .keep();
        let mut owned = HashSet::new();
        if let Some(paths) = load_path {
            let path = directory.join("load-path.json");
            if let Err(error) = fs::write(&path, serde_json::to_vec(paths)?) {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_dir(&directory);
                return Err(error.into());
            }
            owned.insert(path);
        }
        let mut command = Command::new(ruby);
        command
            .env_remove("SONICOP_RUNTIME_HANDOFF")
            .env_remove("SONICOP_RUNTIME_HANDOFF_TOKEN")
            .args(["-e", script])
            .arg(&directory)
            .env(
                "SONICOP_RUNTIME_SEED_UNQUALIFIED",
                if seed_unqualified { "1" } else { "0" },
            )
            .env(
                "SONICOP_RUNTIME_SEED_DEFAULT",
                if seed_default { "1" } else { "0" },
            )
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if clean {
            for key in [
                "RUBYOPT",
                "RUBYLIB",
                "RUBOCOP_OPTS",
                "RUBOCOP_TARGET_RUBY_VERSION",
                "NO_COLOR",
            ] {
                command.env_remove(key);
            }
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                for path in &owned {
                    let _ = fs::remove_file(path);
                }
                let _ = fs::remove_dir(&directory);
                return Err(error).with_context(|| {
                    format!(
                        "could not start Ruby extension runtime: {}",
                        Path::new(ruby).display()
                    )
                });
            }
        };
        Ok(Self {
            child,
            directory,
            owned,
            sequence: 0,
            boot: None,
            finished: None,
        })
    }

    fn finish(&mut self, parent_status: i32) -> Result<i32> {
        let result = self.finish_inner(parent_status);
        if result.is_err() {
            // 壊れたcloseをDropから再送すると、欠けたsequenceを待つ子を再度60秒待ってしまう。
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.finished = Some(2);
        }
        result
    }

    fn finish_inner(&mut self, parent_status: i32) -> Result<i32> {
        if let Some(status) = self.finished {
            return Ok(status);
        }
        // 応答前に終了したchildの0や、timeout後のkill codeで親の失敗を成功へ変えない。
        if self.child.try_wait()?.is_some() {
            self.finished = Some(parent_status);
            return Ok(parent_status);
        }
        self.write_request(&json!({"action":"close", "status":parent_status}))?;
        let response = self
            .directory
            .join(format!("response-{}.json", self.sequence));
        self.owned.insert(response.clone());
        self.owned.insert(
            self.directory
                .join(format!("response-{}.json.pending", self.sequence)),
        );
        let start = Instant::now();
        let mut acknowledged = false;
        loop {
            acknowledged |= close_acknowledged(&response)?;
            if let Some(status) = self.child.try_wait()? {
                // 最初のreadとwaitの間にACK公開→終了しても、正当なat_exit statusを捨てない。
                acknowledged |= close_acknowledged(&response)?;
                let status = if acknowledged {
                    status.code().unwrap_or(2)
                } else {
                    parent_status
                };
                self.finished = Some(status);
                return Ok(status);
            }
            if start.elapsed() >= DEADLINE {
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.finished = Some(2);
                bail!("Ruby extension runtime finalization did not finish within 60 seconds");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn boot(&mut self) -> Result<Json> {
        if let Some(boot) = &self.boot {
            return Ok(boot.clone());
        }
        let path = self.directory.join("boot.json");
        self.owned.insert(path.clone());
        self.owned.insert(self.directory.join("boot.json.pending"));
        let result = self.wait_response(&path)?;
        if result.get("error").is_none()
            && result.get("ready").and_then(Json::as_bool) != Some(true)
        {
            bail!("invalid Ruby extension runtime handshake");
        }
        self.boot = Some(result.clone());
        Ok(result)
    }

    fn write_request(&mut self, value: &Json) -> Result<PathBuf> {
        self.sequence += 1;
        let request = self
            .directory
            .join(format!("request-{}.json", self.sequence));
        let pending = self
            .directory
            .join(format!("request-{}.pending", self.sequence));
        self.owned.insert(request.clone());
        self.owned.insert(pending.clone());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        drop(file);
        fs::rename(&pending, &request)?;
        Ok(request)
    }

    fn request(&mut self, value: &Json) -> Result<Json> {
        let request = self.write_request(value)?;
        let response = self
            .directory
            .join(format!("response-{}.json", self.sequence));
        self.owned.insert(response.clone());
        self.owned.insert(
            self.directory
                .join(format!("response-{}.json.pending", self.sequence)),
        );
        let value = self.wait_response(&response)?;
        let _ = fs::remove_file(&request);
        let _ = fs::remove_file(&response);
        Ok(value)
    }

    fn wait_response(&mut self, path: &Path) -> Result<Json> {
        let start = Instant::now();
        loop {
            match fs::read(path) {
                Ok(bytes) => {
                    return serde_json::from_slice(&bytes)
                        .context("invalid Ruby extension runtime response");
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            if let Some(status) = self.child.try_wait()? {
                bail!("Ruby extension runtime exited before its response ({status})");
            }
            if start.elapsed() >= DEADLINE {
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!("Ruby extension runtime did not respond within 60 seconds");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn close_acknowledged(path: &Path) -> Result<bool> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let value: Json = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        value.get("closing").and_then(Json::as_bool) == Some(true),
        "invalid Ruby extension close acknowledgement"
    );
    Ok(true)
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if self.finished.is_none() && self.finish(0).is_err() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        // Nativeが生成したentryだけをunlinkし、拡張が置いた未知fileを再帰削除しない。
        for path in &self.owned {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_zero_before_response_cannot_turn_a_parent_error_into_success() {
        const CHILD: &str = r#"
require 'json'
root = ARGV.fetch(0)
File.binwrite(File.join(root, 'boot.pending'), JSON.generate({ready: true}))
File.rename(File.join(root, 'boot.pending'), File.join(root, 'boot.json'))
sleep(0.005) until File.file?(File.join(root, 'request-1.json'))
exit 0
"#;
        let ruby = std::ffi::OsStr::new("ruby");
        if Command::new(ruby)
            .args(["-e", "exit 0"])
            .env_remove("RUBYOPT")
            .env_remove("RUBYLIB")
            .status()
            .is_err()
        {
            assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
            eprintln!("未計測: transportの実Rubyがありません");
            return;
        }
        let mut bridge = Bridge::spawn_script(ruby, CHILD, false, false, true, None).unwrap();
        let directory = bridge.directory.clone();
        bridge.boot().unwrap();
        assert!(bridge.request(&json!({"action":"snapshot"})).is_err());
        assert_eq!(bridge.finish(2).unwrap(), 2);
        drop(bridge);
        assert!(!directory.exists());
    }
}
