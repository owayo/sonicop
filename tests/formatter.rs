//! フォーマッタ出力を本家 RuboCop 1.89.0 の実出力に固定する回帰テスト。
//!
//! `tests/formatter/<シナリオ>.<フォーマッタ>` は本家をそのまま走らせて得た
//! バイト列で、sonicop の出力を写したものは 1 つも無い。作業ディレクトリの
//! 絶対パスだけ `{root}` に置き換えてある (`emacs` / `files` / `junit` の
//! failure テキストが絶対パスを出すため)。
//!
//! 対象 cop は [`ONLY`] に固定する。フォーマッタの検証で見たいのは
//! **描画** であって cop の網羅度ではないので、cop が増減してもこのテストは
//! 揺れない。本家との突合は次で再現できる (差分が出れば異常):
//!
//! ```text
//! rubocop --force-default-config --cache false --only <ONLY> <flags> -f <fmt>
//! sonicop --force-default-config --cache false --only <ONLY> <flags> -f <fmt>
//! ```
//!
//! **Unix 限定。** フィクスチャは macOS で本家を走らせて得たバイト列なので、絶対パスの綴りも
//! パス区切りもその OS のものが焼き付いている。Windows では出力が `\` 区切りになり、
//! `canonicalize` が `\\?\` を前置するため `{root}` に畳めず 7 本すべてが落ちる。区切りだけを
//! 機械的に潰して比べることはできるが、それは「区切り以外は本家と同じ」という仮定を
//! 検証せずに置くことになる。Windows で本家を走らせてフィクスチャを別に取るまで、
//! この回帰は Unix で守る (フォーマッタの描画そのものは OS に依存しない)。

#![cfg(unix)]

mod support;

use std::fs;
use std::path::Path;

use support::diff::unified;
use support::project::{command, project_without_pinned_ruby};

/// フィクスチャが踏む cop。sonicop と本家で一致することを確認済みのものだけを並べる。
const ONLY: &str = "Layout/TrailingWhitespace,Lint/UselessAssignment,Naming/FileName,\
     Style/FormatStringToken,Style/FrozenStringLiteralComment,Style/StringLiterals,Style/WordArray";

/// 主シナリオ。1 本で次を踏む:
///
/// - `alpha.rb` — ソース中の `<` `>` `&` (html のエスケープ)、行末空白、
///   空白だけの行に出る offense (引用する行が無いケース)
/// - `beta/gamma.rb` — 下位ディレクトリのパスと、行をまたぐレンジ (省略記号)
/// - `tabbed.rb` — タブ字下げ (キャレット行がタブをそのまま持ち越す)
/// - `fmt.rb` — メッセージ側に `%` `<` `>` を含む cop (github の `%25`、
///   junit / html のエスケープ)
/// - `clean.rb` — offense の無いファイル (節ごと飛ばす経路)
fn build_main(root: &Path) {
    fs::create_dir_all(root.join("beta")).expect("beta を作れなかった");
    write(root, "alpha.rb", "x = \"a<b>&c\"\ny = 1  \n   \n");
    write(
        root,
        "beta/gamma.rb",
        "# frozen_string_literal: true\n\nz = [\n  'a', 'b'\n]\nputs z\n",
    );
    write(
        root,
        "tabbed.rb",
        "# frozen_string_literal: true\n\ndef run\n\tvalue = 1\nend\n",
    );
    write(
        root,
        "fmt.rb",
        "# frozen_string_literal: true\n\nformat('%s %s', 1, 2)\n",
    );
    write(
        root,
        "clean.rb",
        "# frozen_string_literal: true\n\nputs 1\n",
    );
}

/// ファイル全体に対する offense (`Naming/FileName`) を出すシナリオ。
///
/// `add_global_offense` の offense は範囲を持たず、`Offense::NO_LOCATION` の
/// 擬似レンジを抱える。junit はそれを `Struct#to_s` のまま failure テキストへ
/// 書き、html は空のソース行を見て `<pre>` ごと落とす。
fn build_global(root: &Path) {
    write(
        root,
        "BadName.rb",
        "# frozen_string_literal: true\n\nputs 1\n",
    );
    write(
        root,
        "clean.rb",
        "# frozen_string_literal: true\n\nputs 2\n",
    );
}

fn write(root: &Path, name: &str, contents: &str) {
    fs::write(root.join(name), contents)
        .unwrap_or_else(|error| panic!("{name} を書けなかった: {error}"));
}

/// シナリオを 1 フォーマッタ分だけ走らせ、フィクスチャと突き合わせる。
///
/// フォーマッタごとに木を組み直す。`-A` のシナリオは走らせるたびにファイルを
/// 書き換えるので、使い回すと 2 つめのフォーマッタが訂正済みの木を見てしまう。
fn assert_fixture(
    scenario: &str,
    build: fn(&Path),
    flags: &[&str],
    formatter: &str,
    targets: &[&str],
) {
    let directory = project_without_pinned_ruby(&[]);
    let root = directory.path();
    build(root);

    let output = command(root)
        .args(["--force-default-config", "--cache", "false", "--only", ONLY])
        .args(flags)
        .args(["--format", formatter])
        .args(targets)
        .assert()
        .get_output()
        .stdout
        .clone();
    let actual = String::from_utf8(output).expect("出力が UTF-8 でなかった");

    // 絶対パスを出すフォーマッタがあるので、フィクスチャと同じ `{root}` に畳む。
    // macOS の tempdir は `/var/...` だがプロセスの cwd は `/private/var/...` に
    // 解決されるため、両方の綴りを潰す。
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let actual = actual
        .replace(&canonical.to_string_lossy().into_owned(), "{root}")
        .replace(&root.to_string_lossy().into_owned(), "{root}");

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/formatter")
        .join(format!("{scenario}.{formatter}"));
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} を読めなかった: {error}", path.display()));

    assert!(
        actual == expected,
        "{scenario} / {formatter} が本家 1.89.0 の出力と違う\n{}",
        unified(&expected, &actual)
    );
}

/// `main` シナリオを全フォーマッタで固定する。
#[test]
fn main_scenario_matches_rubocop() {
    for formatter in [
        "github", "markdown", "html", "emacs", "simple", "clang", "tap", "progress", "quiet",
        "files",
    ] {
        assert_fixture("main", build_main, &[], formatter, &["."]);
    }
}

/// `--no-display-cop-names`: cop 名の前置だけが落ち、他は変わらない。
#[test]
fn without_cop_names_matches_rubocop() {
    for formatter in [
        "github", "markdown", "html", "emacs", "simple", "clang", "tap",
    ] {
        assert_fixture(
            "nocopnames",
            build_main,
            &["--no-display-cop-names"],
            formatter,
            &["."],
        );
    }
}

/// `--display-style-guide`: メッセージ末尾に URL が付く。付くのは
/// `MessageAnnotator` を通った本文なので、`[Correctable]` の内側に入る。
#[test]
fn with_style_guide_matches_rubocop() {
    for formatter in ["github", "markdown", "html", "emacs", "simple"] {
        assert_fixture(
            "styleguide",
            build_main,
            &["--display-style-guide"],
            formatter,
            &["."],
        );
    }
}

/// `-A` の後。テキスト系は `[Corrected]` に変わり、github / markdown / html は
/// 状態マーカーを持たないので本文が変わらない。
#[test]
fn after_autocorrect_all_matches_rubocop() {
    for formatter in [
        "github", "markdown", "html", "emacs", "simple", "clang", "tap", "progress",
    ] {
        assert_fixture("corrected", build_main, &["-A"], formatter, &["."]);
    }
}

/// ファイル全体に対する offense。junit は全 cop 分の `<testcase>` を出すので、
/// このフィクスチャが「cop が 1 つ黙っても消えずに成功として残る」ことを担保する。
#[test]
fn global_offense_matches_rubocop() {
    for formatter in ["github", "html", "emacs", "simple", "clang", "tap", "junit"] {
        assert_fixture("global", build_global, &[], formatter, &["."]);
    }
}

/// `./` 付きで指定したときのパス。本家はファイル探索で対象を展開してから
/// `smart_path` に渡すので、`./alpha.rb` は `alpha.rb` として出る。
#[test]
fn dot_slash_targets_match_rubocop() {
    for formatter in ["github", "simple", "clang", "tap", "emacs"] {
        assert_fixture(
            "dotslash",
            build_main,
            &[],
            formatter,
            &["./alpha.rb", "./beta/gamma.rb"],
        );
    }
}

/// `--display-only-failed` は junit 専用で、offense を出した cop の
/// `<testcase>` だけを残す。`failures` の数は絞る前に数え終えているので変わらない。
#[test]
fn junit_display_only_failed_matches_rubocop() {
    assert_fixture(
        "onlyfailed",
        build_main,
        &["--display-only-failed"],
        "junit",
        &["."],
    );
}
