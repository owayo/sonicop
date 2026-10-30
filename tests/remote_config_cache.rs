//! RemoteConfig の実 CLI と、別 process が読む cache の本文・時刻・条件付き取得を照合する。

use std::fs::{self, File, FileTimes};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";

#[derive(Default)]
struct Response {
    status: u16,
    body: String,
    requests: Vec<(String, Option<String>)>,
}

struct Server {
    url: String,
    state: Arc<Mutex<Response>>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/config.yml", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Response::default()));
        let observed = Arc::clone(&state);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows では listener の非blocking設定を accepted socket も継承する。
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut first = String::new();
                        reader.read_line(&mut first).unwrap();
                        let path = first.split_whitespace().nth(1).unwrap().to_owned();
                        let mut conditional = None;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                                break;
                            }
                            if let Some((key, value)) = line.split_once(':')
                                && key.eq_ignore_ascii_case("If-Modified-Since")
                            {
                                conditional = Some(value.trim().to_owned());
                            }
                        }
                        let (status, body) = {
                            let mut state = observed.lock().unwrap();
                            state.requests.push((path, conditional));
                            (state.status, state.body.clone())
                        };
                        let reason = if status == 304 { "Not Modified" } else { "OK" };
                        write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            state,
            stopped,
            worker: Some(worker),
        }
    }

    fn reply(&self, enabled: bool, crlf: bool) {
        let body = format!("{BASE}Layout/SpaceAroundOperators:\n  Enabled: {enabled}\n");
        let mut state = self.state.lock().unwrap();
        state.status = 200;
        state.body = if crlf {
            body.replace('\n', "\r\n")
        } else {
            body
        };
    }

    fn not_modified(&self) {
        let mut state = self.state.lock().unwrap();
        state.status = 304;
        state.body.clear();
    }

    fn take_requests(&self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.state.lock().unwrap().requests)
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn environment(command: &mut Command, root: &Path) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .env("RUBOCOP_CACHE_ROOT", root.join("cache"))
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let path =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&path);
    environment(&mut command, root);
    let output = command.args(["-e", LOADER]).output();
    if output.as_ref().is_ok_and(|output| output.status.success()) {
        Some(path)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定 Ruby/RuboCop が必要です: {output:?}"
        );
        eprintln!("未計測: 実 Ruby がないため上流 CLI との比較は行いません");
        None
    }
}

#[derive(Debug, PartialEq)]
struct Original {
    bytes: Vec<u8>,
    modified: SystemTime,
    permissions: u32,
}

fn snapshot(path: &Path) -> Original {
    let info = fs::metadata(path).unwrap();
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode()
    };
    #[cfg(windows)]
    let permissions = u32::from(info.permissions().readonly());
    Original {
        bytes: fs::read(path).unwrap(),
        modified: info.modified().unwrap(),
        permissions,
    }
}

struct Project {
    root: PathBuf,
    cache: PathBuf,
    originals: Vec<(PathBuf, Original)>,
}

struct ReadonlyCache {
    path: PathBuf,
    original: fs::Permissions,
}

impl ReadonlyCache {
    fn new(path: &Path) -> Self {
        let original = fs::metadata(path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(path, readonly).unwrap();
        Self {
            path: path.to_owned(),
            original,
        }
    }
}

impl Drop for ReadonlyCache {
    fn drop(&mut self) {
        fs::set_permissions(&self.path, self.original.clone()).unwrap();
    }
}

impl Project {
    fn new(root: PathBuf, url: &str) -> Self {
        fs::create_dir(&root).unwrap();
        let mut originals = Vec::new();
        for (name, contents) in [
            ("Gemfile", String::new()),
            ("a.rb", "x=1\n".to_owned()),
            ("config.yml", format!("{BASE}inherit_from: {url}\n")),
        ] {
            let path = root.join(name);
            fs::write(&path, contents).unwrap();
            originals.push((path.clone(), snapshot(&path)));
        }
        let cache = root
            .join("cache/rubocop_cache")
            .join(format!("config-{:x}.yml", md5::compute(url)));
        Self {
            root,
            cache,
            originals,
        }
    }

    fn run(&self, ruby: Option<&Path>) -> Output {
        let mut command = if let Some(ruby) = ruby {
            let mut command = Command::new(ruby);
            command.args([
                "-e",
                &format!("{LOADER} exit RuboCop::CLI.new.run(ARGV)"),
                "--",
            ]);
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
            command.env("PATH", "");
            command
        };
        environment(&mut command, &self.root);
        let done = command
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--format",
                "json",
                "a.rb",
            ])
            .output()
            .unwrap();
        for (path, before) in &self.originals {
            assert_eq!(
                &snapshot(path),
                before,
                "原本が変わりました: {}",
                path.display()
            );
        }
        done
    }

    fn mtime(&self, time: SystemTime) {
        File::options()
            .write(true)
            .open(&self.cache)
            .unwrap()
            .set_times(FileTimes::new().set_modified(time))
            .unwrap();
    }
}

fn report(done: &Output, expected: i32, offenses: u64) -> Value {
    assert_eq!(
        done.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(
        done.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let mut report: Value = serde_json::from_slice(&done.stdout).unwrap();
    assert_eq!(
        report["summary"],
        serde_json::json!({"offense_count":offenses,"target_file_count":1,"inspected_file_count":1})
    );
    // 製品 identity のみを分け、診断・summary・残る metadata は全項目を比較する。
    for key in [
        "sonicop_version",
        "ruby_engine",
        "ruby_version",
        "ruby_platform",
    ] {
        report["metadata"].as_object_mut().unwrap().remove(key);
    }
    report
}

#[test]
fn cache_survives_processes_and_matches_real_rubocop_updates() {
    let directory = tempfile::tempdir().unwrap();
    let ruby = ruby(directory.path());
    let mut server = Server::new();
    let native = Project::new(directory.path().join("native"), &server.url);
    let reference = ruby
        .as_ref()
        .map(|_| Project::new(directory.path().join("reference"), &server.url));
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
    let conditional = DateTime::<Local>::from(old)
        .format("%a, %d %b %Y %H:%M:%S %z")
        .to_string();
    let mut readonly = Vec::new();

    for step in 0..9 {
        let requests = match step {
            0 => {
                server.reply(true, false);
                1
            }
            1 => {
                server.reply(false, false);
                0
            }
            2 => {
                native.mtime(old);
                if let Some(reference) = &reference {
                    reference.mtime(old);
                }
                1
            }
            3 => {
                native.mtime(old);
                if let Some(reference) = &reference {
                    reference.mtime(old);
                }
                server.not_modified();
                1
            }
            4 => 1,
            5 => {
                server.reply(true, true);
                1
            }
            6 => {
                let future = SystemTime::now() + Duration::from_secs(2 * 86400);
                native.mtime(future);
                if let Some(reference) = &reference {
                    reference.mtime(future);
                }
                0
            }
            7 => {
                server.stop();
                0
            }
            8 => {
                readonly.push(ReadonlyCache::new(&native.cache));
                if let Some(reference) = &reference {
                    readonly.push(ReadonlyCache::new(&reference.cache));
                }
                0
            }
            _ => unreachable!(),
        };
        let offenses = if [2, 3, 4].contains(&step) { 0 } else { 1 };
        let before = native.cache.exists().then(|| snapshot(&native.cache));
        let actual = report(&native.run(None), offenses as i32, offenses);
        let native_requests = server.take_requests();
        assert_eq!(native_requests.len(), requests, "step {step}");
        if step == 0 {
            assert_eq!(native_requests[0], ("/config.yml".into(), None));
        }
        if [2, 3, 4, 5].contains(&step) {
            assert_eq!(
                native_requests[0],
                ("/config.yml".into(), Some(conditional.clone()))
            );
        }
        if [1, 3, 4, 6, 7, 8].contains(&step) {
            assert_eq!(snapshot(&native.cache), before.unwrap(), "step {step}");
        }
        if let (Some(ruby), Some(reference)) = (&ruby, &reference) {
            let before = reference.cache.exists().then(|| snapshot(&reference.cache));
            assert_eq!(
                report(&reference.run(Some(ruby)), offenses as i32, offenses),
                actual,
                "step {step}"
            );
            assert_eq!(server.take_requests(), native_requests, "step {step}");
            assert_eq!(
                fs::read(&native.cache).unwrap(),
                fs::read(&reference.cache).unwrap(),
                "step {step}"
            );
            if [1, 3, 4, 6, 7, 8].contains(&step) {
                assert_eq!(snapshot(&reference.cache), before.unwrap(), "step {step}");
            }
        }
    }
    drop(readonly);
    // 新鮮な cache の配信停止と、期限切れ後の接続拒否を混同しない。
    native.mtime(old);
    let before = snapshot(&native.cache);
    let done = native.run(None);
    assert_eq!(done.status.code(), Some(2));
    assert!(done.stdout.is_empty());
    assert_eq!(snapshot(&native.cache), before);
    if let (Some(ruby), Some(reference)) = (&ruby, &reference) {
        reference.mtime(old);
        let before = snapshot(&reference.cache);
        let done = reference.run(Some(ruby));
        assert_eq!(done.status.code(), Some(2));
        assert!(done.stdout.is_empty());
        assert_eq!(snapshot(&reference.cache), before);
    }
}

#[cfg(windows)]
#[test]
fn readonly_expired_cache_is_not_replaced_by_a_successful_response() {
    let directory = tempfile::tempdir().unwrap();
    let ruby = ruby(directory.path());
    let server = Server::new();
    server.reply(true, false);
    let native = Project::new(directory.path().join("native"), &server.url);
    let reference = ruby
        .as_ref()
        .map(|_| Project::new(directory.path().join("reference"), &server.url));
    report(&native.run(None), 1, 1);
    if let (Some(ruby), Some(reference)) = (&ruby, &reference) {
        report(&reference.run(Some(ruby)), 1, 1);
    }
    server.take_requests();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
    native.mtime(old);
    let _readonly_native = ReadonlyCache::new(&native.cache);
    if let Some(reference) = &reference {
        reference.mtime(old);
    }
    let _readonly_reference = reference
        .as_ref()
        .map(|project| ReadonlyCache::new(&project.cache));
    server.reply(false, false);
    let before = snapshot(&native.cache);
    let done = native.run(None);
    assert_eq!(done.status.code(), Some(2));
    assert!(done.stdout.is_empty());
    assert_eq!(snapshot(&native.cache), before);
    let header = String::from_utf8(done.stderr)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .replace(&native.root.to_string_lossy().replace('\\', "/"), "<ROOT>");
    assert!(header.starts_with("Permission denied @ rb_sysopen - <ROOT>/cache/rubocop_cache/"));
    let native_requests = server.take_requests();
    assert_eq!(native_requests.len(), 1);
    if let (Some(ruby), Some(reference)) = (&ruby, &reference) {
        let before = snapshot(&reference.cache);
        let done = reference.run(Some(ruby));
        assert_eq!(done.status.code(), Some(2));
        assert!(done.stdout.is_empty());
        assert_eq!(snapshot(&reference.cache), before);
        // Ruby 内部 frame は native 全文一致へ数えず、先頭行と I/O を比較する。
        let reference_header = String::from_utf8(done.stderr)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .replace(
                &reference.root.to_string_lossy().replace('\\', "/"),
                "<ROOT>",
            );
        assert_eq!(header, reference_header);
        assert_eq!(server.take_requests(), native_requests);
    }
}
