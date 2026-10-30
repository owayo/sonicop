//! globをURL判定より先に展開し、HTTP要求・全診断と原本を実本家へ揃える。

#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use serde_json::Value;

fn bind(text: &str, bindings: &BTreeMap<String, String>) -> String {
    bindings.iter().fold(text.to_owned(), |text, (key, value)| {
        text.replace(&format!("<{key}>"), value)
    })
}

fn decode(value: &Value) -> Vec<u8> {
    let (pairs, rest) = value.as_str().unwrap().as_bytes().as_chunks::<2>();
    assert!(rest.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn inspect(row: &Value, fixture: &Value) -> bool {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut bindings = BTreeMap::from([
        ("BASE".to_owned(), base.clone()),
        ("CWD".to_owned(), cwd.to_string_lossy().into_owned()),
        (
            "CWD_FWD".to_owned(),
            cwd.to_string_lossy().replace('\\', "/"),
        ),
    ]);
    for (i, path) in row["routes"].as_object().unwrap().keys().enumerate() {
        bindings.insert(
            format!("HASH_{i}"),
            format!("{:x}", md5::compute(format!("{base}{path}"))),
        );
    }
    let routes: BTreeMap<String, String> = row["routes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, body)| (path.clone(), bind(body.as_str().unwrap(), &bindings)))
        .collect();
    let mut originals = Vec::new();
    for (name, contents) in row["files"].as_object().unwrap() {
        let path = cwd.join(name);
        let bytes = bind(contents.as_str().unwrap(), &bindings).into_bytes();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &bytes).unwrap();
        let info = fs::metadata(&path).unwrap();
        originals.push((path, bytes, info.modified().unwrap(), info.permissions()));
    }
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&requests);
    let stopped = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
                    observed.lock().unwrap().push(path.clone());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                    }
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(cwd)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CONFIG_HOME", cwd.join("empty-config"))
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_CACHE_ROOT",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
    ] {
        command.env_remove(key);
    }
    command
        .env("PATH", "")
        .env("XDG_CACHE_HOME", cwd.join("xdg"));
    for (key, value) in row["env"].as_object().unwrap() {
        command.env(key, bind(value.as_str().unwrap(), &bindings));
    }
    for arg in row["args"].as_array().unwrap() {
        command.arg(arg.as_str().unwrap());
    }
    let output = command.output().unwrap();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let id = row["id"].as_str().unwrap();
    assert_eq!(
        output.status.code().map(i64::from),
        row["exit"].as_i64(),
        "{id}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stderr,
        bind(row["stderr_template"].as_str().unwrap(), &bindings).as_bytes(),
        "{id}"
    );
    let expected = decode(&row["stdout_hex"]);
    if expected.is_empty() {
        assert!(output.stdout.is_empty(), "{id}");
    } else {
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut reference: Value = serde_json::from_slice(&expected).unwrap();
        assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(
            actual["metadata"]["sonicop_version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
        assert_eq!(actual["metadata"]["ruby_version"], "n/a");
        assert_eq!(actual["metadata"]["ruby_platform"], std::env::consts::OS);
        assert_eq!(reference["metadata"]["ruby_engine"], "ruby");
        for key in fixture["metadata_identity_keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            actual["metadata"].as_object_mut().unwrap().remove(key);
            reference["metadata"].as_object_mut().unwrap().remove(key);
        }
        assert_eq!(actual, reference, "{id}");
    }
    let requests = requests.lock().unwrap();
    let expected: Vec<_> = row["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(*requests, expected, "{id}");
    for (path, bytes, mtime, permissions) in originals {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{id}");
        let after = fs::metadata(path).unwrap();
        assert_eq!(after.modified().unwrap(), mtime, "{id}");
        assert_eq!(after.permissions(), permissions, "{id}");
    }
    true
}

#[test]
fn fourteen_actual_cli_conditions_compare_glob_before_http_and_originals() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/inherit_glob_cli.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 14);
    for row in fixture["cases"].as_array().unwrap() {
        assert!(inspect(row, &fixture));
    }
}
