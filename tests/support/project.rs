//! CLI テスト用のヘルパ。サブプロセスを起こす都合上、対象 Ruby と作業
//! ディレクトリをテストごとに閉じ込める責務をここへ集める。

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use assert_cmd::cargo::cargo_bin_cmd;
use serde::Deserialize;
use tempfile::{TempDir, tempdir};

use super::with_target_ruby;

/// CLI テスト既定の TargetRubyVersion。
pub const DEFAULT_TARGET_RUBY: &str = "2.7";

/// `.rubocop.yml` に対象 Ruby を明示した一時プロジェクトを作る。
///
/// リポジトリルートで CLI を走らせると sonicop.gemspec の
/// `required_ruby_version` が対象 Ruby として拾われ、配布メタデータを変えると
/// 無関係なリンタのテストが落ちる。CLI テストは必ず tempdir に閉じ込める。
///
/// `files` に `.rubocop.yml` を含めた場合も `AllCops/TargetRubyVersion` は
/// 上書きされる。別の版を使いたいときは [`project_with_ruby`] を使う。
pub fn project(files: &[(&str, &str)]) -> TempDir {
    project_with_ruby(files, DEFAULT_TARGET_RUBY)
}

/// `.rubocop.yml` を勝手に置かない一時プロジェクト。対象 Ruby の解決そのもの
/// (gemspec / `.ruby-version` からの推定) を検証するケース専用。
pub fn project_without_pinned_ruby(files: &[(&str, &str)]) -> TempDir {
    let directory = tempdir().expect("一時ディレクトリを作れなかった");
    for (name, contents) in files {
        write(directory.path(), name, contents);
    }
    directory
}

pub fn project_with_ruby(files: &[(&str, &str)], target_ruby: &str) -> TempDir {
    let directory = tempdir().expect("一時ディレクトリを作れなかった");
    let configured = files
        .iter()
        .find(|(name, _)| *name == ".rubocop.yml")
        .map(|(_, contents)| *contents);
    write(
        directory.path(),
        ".rubocop.yml",
        &with_target_ruby(configured, target_ruby),
    );
    for (name, contents) in files {
        if *name == ".rubocop.yml" {
            continue;
        }
        write(directory.path(), name, contents);
    }
    directory
}

/// 一時プロジェクトを作業ディレクトリにした sonicop を用意する。
///
/// `RUBOCOP_TARGET_RUBY_VERSION` は設定より優先されるため、開発者のシェルに
/// 残っていてもテストが揺れないよう毎回落とす。
pub fn command(directory: &Path) -> Command {
    let mut command = cargo_bin_cmd!("sonicop");
    command
        .current_dir(directory)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION");
    command
}

/// 標準入力のソースを cop 指定で検査し、JSON 出力を返す。
pub fn lint_stdin(directory: &Path, only: &str, source: &str) -> Assert {
    command(directory)
        .args(["--stdin", "example.rb", "--format", "json", "--only", only])
        .write_stdin(source.to_owned())
        .assert()
}

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("一時プロジェクトのディレクトリを作れなかった");
    }
    fs::write(&path, contents).expect("一時プロジェクトへ書き込めなかった");
}

/// `--format json` の出力。sonicop / RuboCop が足すフィールドが増えても
/// 落ちないよう、必要なものだけを取る。
#[derive(Debug, Deserialize)]
pub struct Report {
    pub files: Vec<FileEntry>,
    pub summary: Summary,
}

#[derive(Debug, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub offenses: Vec<JsonOffense>,
}

#[derive(Debug, Deserialize)]
pub struct JsonOffense {
    pub severity: String,
    pub message: String,
    pub cop_name: String,
    pub corrected: bool,
    pub correctable: bool,
    pub location: JsonLocation,
}

#[derive(Debug, Deserialize)]
pub struct JsonLocation {
    pub start_line: usize,
    pub start_column: usize,
    pub last_line: usize,
    pub last_column: usize,
    pub length: usize,
}

#[derive(Debug, Deserialize)]
pub struct Summary {
    pub offense_count: usize,
    pub target_file_count: usize,
    pub inspected_file_count: usize,
}

/// 整形済み JSON を生文字列で照合すると serde のフィールド順や空白の変更で
/// 全滅するため、構造へ起こしてから比較する。
pub fn report(stdout: &[u8]) -> Report {
    serde_json::from_slice(stdout).unwrap_or_else(|error| {
        panic!(
            "JSON 出力を読めなかった: {error}\n--- stdout ---\n{}",
            String::from_utf8_lossy(stdout)
        )
    })
}

/// 全ファイル分の offense を `(cop_name, line, column, message)` で返す。
pub fn offense_tuples(stdout: &[u8]) -> Vec<(String, usize, usize, String)> {
    report(stdout)
        .files
        .iter()
        .flat_map(|file| &file.offenses)
        .map(|offense| {
            (
                offense.cop_name.clone(),
                offense.location.start_line,
                offense.location.start_column,
                offense.message.clone(),
            )
        })
        .collect()
}

/// offense 一覧を `(cop_name, line, column, message)` で突き合わせる。
pub fn assert_offenses(stdout: &[u8], expected: &[(&str, usize, usize, &str)]) {
    let expected: Vec<(String, usize, usize, String)> = expected
        .iter()
        .map(|(cop_name, line, column, message)| {
            (
                (*cop_name).to_owned(),
                *line,
                *column,
                (*message).to_owned(),
            )
        })
        .collect();
    assert_eq!(
        offense_tuples(stdout),
        expected,
        "JSON の offense 一覧が期待と違う"
    );
}

pub fn offenses(stdout: &[u8]) -> Vec<JsonOffense> {
    report(stdout)
        .files
        .into_iter()
        .flat_map(|file| file.offenses)
        .collect()
}
