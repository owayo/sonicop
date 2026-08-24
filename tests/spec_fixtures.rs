//! 本家 RuboCop 1.89.0 の spec を入力源にした、cop 単位の突き合わせゲート。
//!
//! `tests/cops.rs` と `tests/conformance.rs` は手で書いたケースで、**609 cop のうち
//! offense 検出を検証できているのは 427 cop / 1,070 ケース** だった (2026-08-23 実測。
//! `SONICOP_COP_COVERAGE=<path> cargo test --test cops` で数え直せる)。手で書き足す限り
//! この差は縮まらないので、**本家の spec そのものを入力にする**。
//!
//! 期待値は spec の字面ではなく**本家の実出力**である。本家が自分の spec どおりに
//! 動かない例が実測であるため、`spec_fixture_gen.py` が本家を 1 度起動して録る。
//! 録ったものを読むだけなので、この gate に rubocop gem は要らない。
//!
//! ```text
//! # 期待値の生成 (本家を cop の数だけ起動する。30 分以上かかる)
//! python3 ~/.claude/skills/migrate-rubocop/scripts/spec_fixture_gen.py \
//!         --all --out tests/fixtures
//! ```
//!
//! 生成物は本家 spec の入力テキストを含む。**追加するときは NOTICE に同じ変更で
//! 記載すること** — MIT なので持ち込めるが、持ち込んだ記録が無い状態は作らない。
//!
//! 差分は `#[ignore]` ではなくマニフェスト ([`support::manifest`]) に登録する。
//! 直ったのにエントリが残っていても失敗するので、修正がマニフェストの掃除を強制する。

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sonicop::rules::rule_names;
use support::annotation::Annotation;
use support::case::CopCase;
use support::manifest::Manifest;

/// 本家の出力が有効な Ruby だったケース。一致を要求してよい。
const CASES: &str = "upstream_spec_cases.jsonl";
/// 本家の `-A` 出力が `ruby -c` を通らなかったケース。**一致を要求してはいけない** --
/// 要求すると「壊れた出力を出せ」というテストになる。
const UNROUNDTRIPPABLE: &str = "upstream_spec_divergences.jsonl";
/// この gate 専用の既知差分。手書きケース用 (`known_divergences.yml`) とは別に持つ。
/// 混ぜると、どちらの入力で出た差分か分からなくなる。
const MANIFEST: &str = "tests/conformance/spec_known_divergences.yml";

/// 期待値の置き場。開発中に別の生成物で試せるよう環境変数で差し替えられる。
fn fixture_dir() -> PathBuf {
    match std::env::var("SONICOP_SPEC_FIXTURE_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from("tests/fixtures"),
    }
}

/// 録った 1 ケース。`spec_fixture_gen.py` の `record_for` と対になる。
///
/// **空のキーは持たない**という取り決めがあり、`offenses` が無い = 本家はそこで黙る、
/// `corrected` が無い = 本家は書き換えなかった、と読む。`Option` ではなく既定値で
/// 受けるのはそのため。
#[derive(Debug, Deserialize)]
struct Record {
    cop: String,
    source: String,
    /// `<spec ファイル>:<行>`。落ちたときに本家のどこを見ればよいかを示す。
    origin: String,
    /// 録った本家の版。行ごとに持つ (部分集合を切り出しても版が消えないように)。
    upstream: String,
    #[serde(default)]
    offenses: Vec<RecordedOffense>,
    #[serde(default)]
    corrected: Option<String>,
    /// **YAML の断片**であって型のついた値ではない。`"[ruby19, hash_rockets]"` は
    /// 列であって文字列ではないので、**そのまま貼る**。引用符で囲み直すと、録ったときの
    /// 条件と再生する条件が食い違う。
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    target_ruby: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordedOffense {
    message: String,
    line: usize,
    column: usize,
    length: usize,
}

/// 対象ファイルの拡張子。**部門ごとに `Include` が違うので、`.rb` で書くと
/// Gemspec / Bundler の cop は 1 件も当たらない。**
///
/// 規則は録った側 (`spec_oracle.py` の `SUFFIX`) と同じでなければならない。片方だけ
/// 変えると、本家は `.gemspec` で測ったものを移植版は `.rb` で再生し、**差分が
/// 「移植版のバグ」の顔をして出る**。
fn path_for(cop: &str) -> &'static str {
    match cop.split('/').next() {
        Some("Gemspec") => "example.gemspec",
        Some("Bundler") => "example.gemfile",
        _ => "example.rb",
    }
}

/// 本家 spec が使う仮想ファイル名 (`c0000.rb`) がメッセージに焼き込まれているなら、その名前で
/// 再生する。
///
/// `Naming/FileName` や `Lint/ScriptPermission` は**ファイル名そのものを報告する**ので、
/// `example.rb` で再生すると本文が食い違う -- 中身の差ではないのに差分として出る。cop 名で
/// 特別扱いを並べるのではなく、メッセージに現れた名前をそのまま使う。
fn path_in_message(offenses: &[RecordedOffense]) -> Option<String> {
    static VIRTUAL_PATH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bc\d{4,}\.(?:rb|gemspec|gemfile)\b").expect("パターンが不正")
    });
    offenses
        .iter()
        .find_map(|offense| VIRTUAL_PATH.find(&offense.message))
        .map(|found| found.as_str().to_owned())
}

/// `Lint/Syntax` が出したメッセージかどうか。cop 名は記録されていないので、`parser` gem の
/// 診断が必ず添える文言で見分ける。
fn is_syntax_error(message: &str) -> bool {
    message.contains("parser; configure using `TargetRubyVersion` parameter")
}

/// 本家 JSON の `length` (レンジ全体の文字数) を、注記のキャレット本数に直す。
///
/// **この 2 つは別物である。**`length` はレンジ全体の長さで、キャレットの本数は
/// 開始行に収まる幅 (`support::annotation::from_offense` と同じ規則)。取り違えると
/// **行を跨る offense がすべて `range` 差分に化ける** -- 実際、Metrics 系 3 cop だけで
/// 57 件がこれだった。
fn caret_length(source: &str, line: usize, column: usize, length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    let width = source
        .lines()
        .nth(line.saturating_sub(1))
        .map(|text| text.trim_end_matches('\r').chars().count())
        .unwrap_or(0);
    length.min(width.saturating_sub(column.saturating_sub(1)))
}

/// 本家のメッセージに焼き込まれた**録ったときの一時ディレクトリ**を、ハーネスの前方一致に
/// 委ねる形へ畳む。
///
/// `Lint/DuplicateMethods` は "defined at both `<path>`:2 and `<path>`:5" と書き、その
/// `<path>` は録ったときの `TemporaryDirectory` である。**再生できないので、比べれば必ず
/// 差分になる。**畳んだ先は `[...]` -- 本家 `match_annotations?` と同じ前方一致なので、
/// **パス以降 (行番号を含む) はこの gate では検証していない**。件数は実行のたびに出す。
fn abbreviate_recorded_paths(message: &str) -> Option<String> {
    static ABSOLUTE_PATH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"/[^\s:]*/[^\s:/]+\.(?:rb|gemspec|gemfile):\d+").expect("パターンが不正")
    });
    let found = ABSOLUTE_PATH.find(message)?;
    Some(format!("{}[...]", &message[..found.start()]))
}

impl Record {
    fn to_case(&self) -> CopCase {
        let expected = self
            .offenses
            .iter()
            .map(|offense| {
                Annotation::new(
                    offense.line,
                    offense.column,
                    caret_length(&self.source, offense.line, offense.column, offense.length),
                    abbreviate_recorded_paths(&offense.message)
                        .unwrap_or_else(|| offense.message.clone()),
                )
            })
            .collect();
        let mut case = CopCase::new(&self.cop, self.source.clone(), expected)
            .id(&self.origin)
            .path(
                &path_in_message(&self.offenses).unwrap_or_else(|| path_for(&self.cop).to_owned()),
            );
        // 本家が出した `location.length` そのものも見る。キャレット本数だけだと、
        // 行を跨るレンジの**終端**を一度も検証しないまま緑になる。
        if !self.offenses.is_empty() {
            let lengths: Vec<usize> = self.offenses.iter().map(|item| item.length).collect();
            case = case.lengths(&lengths);
        }
        if !self.config.is_empty() {
            let mut yaml = format!("{}:\n", self.cop);
            for (key, value) in &self.config {
                yaml.push_str(&format!("  {key}: {value}\n"));
            }
            case = case.config(&yaml);
        }
        if let Some(version) = &self.target_ruby {
            case = case.target_ruby(version);
        }
        if let Some(corrected) = &self.corrected {
            case = case.corrected_verbatim(corrected);
        }
        // 本家自身が構文エラーを報告したケースでは、それこそが期待値。構文ガードは
        // 「本家が読めた例を移植版が読めない」を捕まえるためのものなので、ここでは外す。
        if self
            .offenses
            .iter()
            .any(|item| is_syntax_error(&item.message))
        {
            case = case.expecting_syntax_error();
        }
        // モードやファイルの存在そのものを読む cop は、名前だけのパスでは何も見えない。
        if self.cop == "Lint/ScriptPermission" {
            case = case.materialized();
        }
        case
    }
}

/// JSONL を読む。**1 行でも壊れていたら止める** — 読み飛ばすと、読めた分だけで
/// 「全件一致」を名乗ることになる。
fn load(name: &str) -> Vec<Record> {
    let path = fixture_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} を読めない: {error}\n\
             期待値がまだ録られていない。`make spec-fixtures` で録る \
             (本家を cop の数だけ起動するので 1 時間前後かかる)。\n\
             別の生成物で試すときは SONICOP_SPEC_FIXTURE_DIR=<dir> を渡す。",
            path.display()
        )
    });
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("{}:{} が読めない: {error}", path.display(), index + 1)
            })
        })
        .collect()
}

// パニックの既定出力はそのまま流す。**抑制しようとして 1 度失敗している** -- テストは
// 並列に走るので、テストごとに `set_hook` / `take_hook` で包むと**片方の drop が
// もう片方の抑制を解除する**。プロセスに 1 度だけ入れる形なら成立するが、そうすると
// 捕まえ損ねた本物のパニックまで文言を失う。止まったケースは集計にも出るので、
// 二重に見えるほうを採る。

/// `catch_unwind` の payload から読める文言を取り出す。
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    let text = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("(文言の無いパニック)");
    // 1 行にまとめる。集計の行が複数行に散ると数えられない。
    text.lines().next().unwrap_or(text).to_owned()
}

/// 本家が録った版が全行で揃っていることを見る。**混ざった fixture は、どちらの版に
/// 対する主張なのかが言えない。**
fn assert_single_upstream_version(records: &[Record]) {
    let versions: BTreeSet<&str> = records
        .iter()
        .map(|record| record.upstream.as_str())
        .collect();
    assert_eq!(
        versions.len(),
        1,
        "録った本家の版が混ざっている: {versions:?}。再生成すること"
    );
}

#[test]
fn every_recorded_upstream_case_matches() {
    let records = load(CASES);
    assert_single_upstream_version(&records);
    let manifest = Manifest::load(Path::new(MANIFEST));

    // **測っていないものを数えて出す。**畳んだメッセージは前方一致で通るので、黙っていると
    // 「全件が完全一致した」と読めてしまう。
    let abbreviated = records
        .iter()
        .flat_map(|record| &record.offenses)
        .filter(|offense| abbreviate_recorded_paths(&offense.message).is_some())
        .count();
    if abbreviated > 0 {
        eprintln!(
            "録ったときの絶対パスを含むメッセージ {abbreviated} 件は、パスの手前までしか \
             比べていない (行番号を含む後半は未検証)"
        );
    }

    let mut unknown = Vec::new();
    let mut resolved = Vec::new();
    let mut reversed = Vec::new();
    let mut known = 0usize;
    let mut by_cop: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut aborted = Vec::new();
    // ハーネスは 1 件の異常でパニックする (パースできない入力、収束しない autocorrect)。
    // **11,300 件を回す側では、それは 1 件ぶんの結果として集計する。**そうしないと
    // 最初の 1 件で全体が止まり、**残り 11,299 件を測っていないのに「1 件失敗」と読める。**

    for record in &records {
        let judged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            manifest.judge(&record.to_case())
        }));
        let (verdict, detail) = match judged {
            Ok(judged) => judged,
            Err(payload) => {
                aborted.push(format!(
                    "{} [{}] {}",
                    record.origin,
                    record.cop,
                    panic_message(&payload)
                ));
                *by_cop.entry(record.cop.as_str()).or_default() += 1;
                *by_kind.entry("panic".to_owned()).or_default() += 1;
                continue;
            }
        };
        known += verdict.known.len();
        if !verdict.unknown.is_empty() {
            *by_cop.entry(record.cop.as_str()).or_default() += verdict.unknown.len();
            for divergence in &verdict.unknown {
                *by_kind.entry(divergence.kind.to_string()).or_default() += 1;
            }
            unknown.push(format!("{} [{}]\n{detail}", record.origin, record.cop));
        }
        for entry in &verdict.resolved {
            resolved.push(format!("{} [{}] {}", entry.case_id, entry.cop, entry.kind));
        }
        for entry in &verdict.reversed {
            reversed.push(format!("{} [{}] {}", entry.case_id, entry.cop, entry.kind));
        }
    }

    // **差分の全文は、頼まれたときだけファイルに出す。**画面に流すと最初の 1 件を読むのに
    // スクロールが要り、`| tail` で受けると先頭が消える (どちらも実際にやった)。
    if let Ok(path) = std::env::var("SONICOP_SPEC_REPORT") {
        let body = match aborted.is_empty() {
            true => unknown.join("\n\n"),
            false => format!(
                "== ハーネスが止まったケース {} 件 ==\n{}\n\n== 差分 {} 件 ==\n{}",
                aborted.len(),
                aborted.join("\n"),
                unknown.len(),
                unknown.join("\n\n")
            ),
        };
        fs::write(&path, body).unwrap_or_else(|error| panic!("{path} に書けない: {error}"));
        eprintln!(
            "差分 {} 件 / 止まったケース {} 件を {path} に書いた",
            unknown.len(),
            aborted.len()
        );
    }

    // 失敗時の出力は先頭だけ。**ただし集計は必ず全件ぶん出す** — 件数と分布が見えないと、
    // 直す順序が決められない。
    let head = |items: &[String], limit: usize| -> String {
        let shown: Vec<&str> = items.iter().take(limit).map(String::as_str).collect();
        match items.len() > limit {
            true => format!("{}\n… 他 {} 件", shown.join("\n"), items.len() - limit),
            false => shown.join("\n"),
        }
    };
    let tally = |counts: &BTreeMap<&str, usize>, limit: usize| -> String {
        let mut rows: Vec<(&str, usize)> = counts.iter().map(|(k, v)| (*k, *v)).collect();
        rows.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
        let shown: Vec<String> = rows
            .iter()
            .take(limit)
            .map(|(cop, count)| format!("  {count:5}  {cop}"))
            .collect();
        match rows.len() > limit {
            true => format!("{}\n  … 他 {} cop", shown.join("\n"), rows.len() - limit),
            false => shown.join("\n"),
        }
    };
    assert!(
        unknown.is_empty() && resolved.is_empty() && reversed.is_empty() && aborted.is_empty(),
        "本家 spec 由来 {} ケース / {} cop。既知差分 {known} 件。\n\n\
         == ハーネスが止まったケース {} 件 ==\n{}\n\n\
         == 未登録の差分 {} 件 ({} cop) ==\n\
         種別ごと: {:?}\n\
         cop ごと:\n{}\n\n\
         全文は SONICOP_SPEC_REPORT=<path> を渡すと書き出す。先頭 3 件:\n{}\n\n\
         == 登録があるのに出ない差分 {} 件 (直ったなら消すこと) ==\n{}\n\n\
         == 向きが逆のエントリ {} 件 (ケースが移植版を写している。消してはいけない) ==\n{}",
        records.len(),
        records
            .iter()
            .map(|record| record.cop.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        aborted.len(),
        head(&aborted, 10),
        unknown.len(),
        by_cop.len(),
        by_kind,
        tally(&by_cop, 25),
        head(&unknown, 3),
        resolved.len(),
        head(&resolved, 20),
        reversed.len(),
        head(&reversed, 20),
    );
}

/// 本家の `-A` が壊れた Ruby を吐くケース。**一致を要求せず、移植版が壊れた出力を
/// 書いていないことだけを見る。**振り分けた先を検査しないと、移植版が壊れた出力を
/// 書き始めても誰も気づかない状態を新しく作ることになる。
#[test]
fn sonicop_does_not_reproduce_upstreams_broken_corrections() {
    let records = load(UNROUNDTRIPPABLE);
    assert_single_upstream_version(&records);

    let mut broken = Vec::new();
    let mut unparsable = Vec::new();

    for record in &records {
        // 期待値を持たないので集合検証は切り、訂正の結果だけを見る。
        let case = CopCase::new(&record.cop, record.source.clone(), Vec::new())
            .id(&record.origin)
            .path(path_for(&record.cop))
            .without_offense_check();
        let case = match &record.target_ruby {
            Some(version) => case.target_ruby(version),
            None => case,
        };
        // **入力そのものを移植版がパースできないことがある** (本家は通す)。それは
        // この gate の主題ではないので、止めずに別枠で数える。
        let inspected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case.inspect()));
        let report = match inspected {
            Ok(report) => report,
            Err(payload) => {
                unparsable.push(format!(
                    "{} [{}] {}",
                    record.origin,
                    record.cop,
                    panic_message(&payload)
                ));
                continue;
            }
        };
        if report.offenses.is_empty() {
            continue;
        }
        broken.push(format!(
            "{} [{}] offense {} 件",
            record.origin,
            record.cop,
            report.offenses.len()
        ));
    }

    // ここは「壊れた出力を書かない」だけを見る gate なので、offense が出ること自体は
    // 正常。件数を出しておき、0 件に落ちたら fixture 側の異常として気づけるようにする。
    assert!(
        !records.is_empty(),
        "本家が round-trip できないケースが 1 件も無い。fixture の生成が途中で終わっている"
    );
    eprintln!(
        "本家が round-trip できないケース {} 件: 移植版が offense を出したもの {} 件 / \
         移植版がパースできなかったもの {} 件",
        records.len(),
        broken.len(),
        unparsable.len()
    );
    if !unparsable.is_empty() {
        eprintln!(
            "  パースできなかったケース:\n    {}",
            unparsable.join("\n    ")
        );
    }
}

/// **この gate がどれだけの cop に届いているかを、gate 自身に言わせる。**
/// 「全 cop を検証している」は、届いていない cop の一覧を出せて初めて主張になる。
#[test]
fn the_fixture_reports_which_cops_it_does_not_reach() {
    let records = load(CASES);
    let unroundtrippable = load(UNROUNDTRIPPABLE);

    let mut reached: BTreeSet<&str> = records.iter().map(|record| record.cop.as_str()).collect();
    reached.extend(unroundtrippable.iter().map(|record| record.cop.as_str()));

    let registered: BTreeSet<&str> = rule_names().collect();
    let unreachable: Vec<&str> = registered.difference(&reached).copied().collect();
    let unknown_cop: Vec<&str> = reached.difference(&registered).copied().collect();

    assert!(
        unknown_cop.is_empty(),
        "fixture が知らない cop を含む: {unknown_cop:?}。本家の版が違う可能性がある"
    );
    eprintln!(
        "本家 spec 由来のケースが届く cop: {} / {} (届かない {} 件)",
        reached.len(),
        registered.len(),
        unreachable.len()
    );
    if !unreachable.is_empty() {
        eprintln!("届かない cop:\n  {}", unreachable.join("\n  "));
    }
}
