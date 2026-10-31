//! inherit_fromのnil除去・継承順・HTTP要求を実RuboCopから先に測る。

use std::collections::BTreeMap;
use std::fs;
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

use serde_json::Value;
use sonicop::config::Config;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    fn new(routes: BTreeMap<String, String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windowsではaccepted socketもlistenerの非blockingを継承する。
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut first = String::new();
                        reader.read_line(&mut first).unwrap();
                        let path = first.split_whitespace().nth(1).unwrap().to_owned();
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                                break;
                            }
                        }
                        seen.lock().unwrap().push(path.clone());
                        let body = routes.get(&path).map_or("", String::as_str);
                        let status = if routes.contains_key(&path) {
                            "200 OK"
                        } else {
                            "404 Not Found"
                        };
                        write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn take_requests(&self) -> Vec<String> {
        std::mem::take(&mut self.requests.lock().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
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
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
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
    let ruby =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&ruby);
    environment(&mut command, root);
    let done = command.args(["-e", LOADER]).output();
    if done.as_ref().is_ok_and(|output| output.status.success()) {
        Some(ruby)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Ruby/RuboCopが必要です: {done:?}"
        );
        eprintln!("未計測: 実Ruby/RuboCopがないためnil継承の比較を成功へ数えません");
        None
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    bytes: Vec<u8>,
    modified: SystemTime,
    mode: u32,
}

fn snapshot(path: &Path) -> Snapshot {
    let info = fs::metadata(path).unwrap();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode()
    };
    #[cfg(windows)]
    let mode = u32::from(info.permissions().readonly());
    Snapshot {
        bytes: fs::read(path).unwrap(),
        modified: info.modified().unwrap(),
        mode,
    }
}

fn command(root: &Path, ruby: &Path, reference: bool, script: &str) -> Command {
    let mut command = if reference {
        let mut command = Command::new(ruby);
        command.args(["-e", &format!("{LOADER}\n{script}\n"), "--"]);
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.env("PATH", "");
        command
    };
    environment(&mut command, root);
    command.env(
        "RUBOCOP_CACHE_ROOT",
        root.join(if reference {
            "reference-cache"
        } else {
            "native-cache"
        }),
    );
    command
}

fn report(output: &Output) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout.first(), Some(&b'{'));
    assert_eq!(output.stdout.last(), Some(&b'}'));
    let mut report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
    assert_eq!(report["summary"]["target_file_count"], 1);
    assert_eq!(report["summary"]["inspected_file_count"], 1);
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
fn null_inheritance_matches_real_cli_and_local_configuration_api() {
    let directory = tempfile::tempdir().unwrap();
    let Some(ruby) = ruby(directory.path()) else {
        return;
    };
    let cases = [
        ("absent", None),
        ("null", Some("null")),
        ("one-null", Some("[null]")),
        ("empty", Some("[]")),
        ("two-nulls", Some("[null, null]")),
        ("string", Some("first.yml")),
        (
            "last-false",
            Some("[null, first.yml, null, second.yml, null]"),
        ),
        (
            "last-true",
            Some("[null, second.yml, null, first.yml, null]"),
        ),
        ("quoted-null", Some("[null, 'null', null]")),
        ("missing", Some("[null, missing.yml, null]")),
    ];
    for remote in [false, true] {
        for (name, inherit) in cases {
            let root = directory.path().join(format!("{remote}-{name}"));
            fs::create_dir(&root).unwrap();
            let document = format!(
                "{BASE}{}",
                inherit.map_or(String::new(), |value| format!("inherit_from: {value}\n"))
            );
            let parents = BTreeMap::from([
                (
                    "first.yml",
                    "Layout/SpaceAroundOperators:\n  Enabled: true\n",
                ),
                (
                    "second.yml",
                    "Layout/SpaceAroundOperators:\n  Enabled: false\n",
                ),
                ("null", "Layout/SpaceAroundOperators:\n  Enabled: true\n"),
            ]);
            let mut routes: BTreeMap<_, _> = parents
                .iter()
                .map(|(path, text)| (format!("/{path}"), (*text).to_owned()))
                .collect();
            routes.insert("/config.yml".to_owned(), document.clone());
            let server = Server::new(routes);
            let config = if remote {
                format!("{BASE}inherit_from: '{}/config.yml'\n", server.url)
            } else {
                document
            };
            let mut files = BTreeMap::from([
                ("Gemfile", Vec::new()),
                ("a.rb", b"x=1\n".to_vec()),
                ("config.yml", config.into_bytes()),
                ("report.json", b"KEEP\0\r\n".to_vec()),
            ]);
            files.extend(
                parents
                    .iter()
                    .map(|(path, text)| (*path, text.as_bytes().to_vec())),
            );
            for (path, bytes) in &files {
                fs::write(root.join(path), bytes).unwrap();
            }
            let originals: Vec<_> = files
                .keys()
                .map(|path| (*path, snapshot(&root.join(path))))
                .collect();
            // 同じ原本で本家を先行し、processごとのcacheを分けてHTTPの取得を省略させない。
            let mut results = Vec::new();
            for reference in [true, false] {
                let output = command(&root, &ruby, reference, "exit RuboCop::CLI.new.run(ARGV)")
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
                let requests = server.take_requests();
                for (path, original) in &originals {
                    assert_eq!(
                        snapshot(&root.join(path)),
                        *original,
                        "{remote} {name} {reference} {path}"
                    );
                }
                results.push((output, requests));
            }
            let (reference, expected_requests) = &results[0];
            let (native, actual_requests) = &results[1];
            assert_eq!(
                native.status.code(),
                reference.status.code(),
                "{remote} {name}: {native:?}"
            );
            assert_eq!(actual_requests, expected_requests, "{remote} {name}");
            if name == "missing" {
                assert_eq!(reference.status.code(), Some(2));
                assert_eq!(native.stdout, reference.stdout, "{remote} {name}");
                assert!(!native.stderr.is_empty() && !reference.stderr.is_empty());
                // HTTP 404の既存の文面差は#260。失敗・要求順・原本保持だけをここで比較する。
                if !remote {
                    // 本家のRuby内部frameは互換対象の先頭行へ混ぜない。
                    assert_eq!(
                        native.stderr.split(|byte| *byte == b'\n').next(),
                        reference.stderr.split(|byte| *byte == b'\n').next(),
                        "{remote} {name}"
                    );
                }
                continue;
            }
            assert!([Some(0), Some(1)].contains(&reference.status.code()));
            assert_eq!(report(native), report(reference), "{remote} {name}");
            if !remote {
                let output = command(&root, &ruby, true, "require 'json'; c = RuboCop::ConfigLoader.configuration_from_file('config.yml'); print JSON.generate(c.for_cop('Layout/SpaceAroundOperators')['Enabled'])").output().unwrap();
                assert!(output.status.success(), "{name}: {output:?}");
                assert!(output.stderr.is_empty(), "{name}: {output:?}");
                let enabled: bool = serde_json::from_slice(&output.stdout).unwrap();
                let config = Config::load(Some(&root.join("config.yml")), &root).unwrap();
                assert_eq!(
                    config.rule_enabled("Layout/SpaceAroundOperators"),
                    enabled,
                    "{name}"
                );
                for (path, original) in &originals {
                    assert_eq!(snapshot(&root.join(path)), *original, "API {name} {path}");
                }
            }
        }
    }
}
