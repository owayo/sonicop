//! cop 1 件を in-process で検証するハーネス。
//!
//! `CopCase` が本家 spec 1 ケース分の中間表現で、キャレット注記のパーサ
//! ([`super::annotation`]) はこの struct を組み立てる層として分離してある。
//! 本家 spec を機械変換するときは注記を通さず [`CopCase::new`] を直接組めばよい。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use sonicop::config::Config;
use sonicop::cop_name::selector_matches;
use sonicop::diagnostic::{FileReport, Severity};
use sonicop::engine::{self, CorrectMode, Selection};
use tempfile::TempDir;

use super::annotation::{self, Annotation};
use super::diff;
use super::divergence::{self, Divergence, Kind};

/// ハーネス既定の TargetRubyVersion。本家 spec の既定 (`TargetRuby::DEFAULT_VERSION`)
/// に合わせてある。明示しないと gemspec や `.ruby-version` の探索がテスト実行時の
/// cwd に依存してしまうため、ハーネスは常にこの値を設定ファイルへ書き込む。
pub const DEFAULT_TARGET_RUBY: &str = "2.7";

/// 検証対象の既定ファイル名。cop の `Include` / `Exclude` は既定で `**/*.rb` を見る。
pub const DEFAULT_PATH: &str = "example.rb";

/// 本家 spec 1 ケース分の中間表現。
#[derive(Clone, Debug)]
pub struct CopCase {
    /// ケースの識別子。既知差分マニフェストの突き合わせキーになるので、
    /// 一度付けたら変えないこと。空なら cop 名を使う。
    pub id: String,
    /// 有効にする cop。`--only` と同じ書式で、部署名 (`Style` 等) も使える。
    pub only: Vec<String>,
    /// 解析対象のパス。`Include` / `Exclude` の判定に効く。
    pub path: String,
    /// 注記を取り除いた素のソース。
    pub source: String,
    /// 期待する offense の集合。`None` は集合を検証しない (correction だけ見る)。
    pub expected: Option<Vec<Annotation>>,
    /// `.rubocop.yml` 相当。**`AllCops/TargetRubyVersion` をここに書いても消える** --
    /// ハーネスは下の [`CopCase::target_ruby`] の値で上書きするので、版を変えたいときは
    /// このフィールドではなく `.target_ruby("3.1")` を使う。
    pub config_yaml: Option<String>,
    /// 検査に使う `TargetRubyVersion`。`config_yaml` の同名の設定より優先される。
    pub target_ruby: String,
    /// autocorrect 後の期待ソース。
    pub corrected: Option<String>,
    /// 全 offense が持つべき severity。
    pub severity: Option<Severity>,
    /// 全 offense が持つべき correctable。
    pub correctable: Option<bool>,
    /// offense の cop 名の期待多重集合。`None` なら `only` に含まれることだけ見る。
    pub cop_names: Option<Vec<String>>,
    /// `only` を `--only` として渡すか。`Lint/RedundantCopDisableDirective` は本家が
    /// `--only` と併用できないので、この cop のケースだけ偽にして、選択を
    /// 「他の実装済み cop を全部 `--except` する」形で表す。
    pub uses_only: bool,
    /// 安全網 (`#41`) を切るか。**本家の出力そのものが `ruby -c` を通らないケース専用。**
    /// 既定は偽 = 安全網は働く。
    pub skip_syntax_guard: bool,
    /// `(start_line, start_column, last_line, last_column)` の期待。
    pub locations: Option<Vec<(usize, usize, usize, usize)>>,
    /// `location.length` の期待。本家は文字数で出す。
    pub lengths: Option<Vec<usize>>,
    pub correct_mode: CorrectMode,
}

impl CopCase {
    /// 素の中間表現を直接組み立てる。本家 spec の機械変換はこちらを使う。
    pub fn new(cop: &str, source: impl Into<String>, expected: Vec<Annotation>) -> Self {
        Self {
            id: String::new(),
            only: vec![cop.to_owned()],
            path: DEFAULT_PATH.to_owned(),
            source: source.into(),
            expected: Some(expected),
            config_yaml: None,
            target_ruby: DEFAULT_TARGET_RUBY.to_owned(),
            corrected: None,
            severity: None,
            correctable: None,
            cop_names: None,
            uses_only: true,
            skip_syntax_guard: false,
            locations: None,
            lengths: None,
            correct_mode: CorrectMode::All,
        }
    }

    /// キャレット注記付きソースから中間表現を作る。ソースは `<<~RUBY` と同じ
    /// 規則で字下げを落とすので、本家 spec をそのまま持ち込める。
    pub fn annotated(cop: &str, annotated: &str) -> Self {
        Self::annotated_with(cop, annotated, &[])
    }

    /// `%{key}` / `^{key}` / `_{key}` を展開してから注記を読む。
    pub fn annotated_with(cop: &str, annotated: &str, replacements: &[(&str, &str)]) -> Self {
        let expanded = annotation::expand(&annotation::dedent(annotated), replacements);
        let parsed = annotation::parse(&expanded);
        Self::new(cop, parsed.source, parsed.annotations)
    }

    /// ソース末尾の改行 1 個を落とす。「最終改行が無い」ことを検証するケースは
    /// 注記行を置く場所が無いため、本家の `chomp:` と同じ逃げ道を用意する。
    pub fn chomp(mut self) -> Self {
        if let Some(chomped) = self.source.strip_suffix('\n') {
            self.source = chomped.to_owned();
        }
        self
    }

    /// offense の集合を検証しない。correction だけを見たいときに使う。
    pub fn without_offense_check(mut self) -> Self {
        self.expected = None;
        self
    }

    pub fn cops(mut self, cops: &[&str]) -> Self {
        self.only = cops.iter().map(|cop| (*cop).to_owned()).collect();
        self
    }

    /// offense の cop 名を多重集合で検証する。複数 cop を有効にしたケース向け。
    pub fn cop_names(mut self, names: &[&str]) -> Self {
        self.cop_names = Some(names.iter().map(|name| (*name).to_owned()).collect());
        self
    }

    /// `(start_line, start_column, last_line, last_column)` を厳密に検証する。
    ///
    /// キャレット注記は本家と同じく先頭行の範囲しか表せないので、複数行に跨る
    /// offense (Metrics 系など) の終端まで固定したいときはこれを併用する。
    pub fn locations(mut self, locations: &[(usize, usize, usize, usize)]) -> Self {
        self.locations = Some(locations.to_vec());
        self
    }

    /// `location.length` を検証する。本家はこれを**文字数**で出す。キャレットは
    /// カラムから導くため単位の違いが見えないので、多バイト文字を含むケースは
    /// これで固定する。
    pub fn lengths(mut self, lengths: &[usize]) -> Self {
        self.lengths = Some(lengths.to_vec());
        self
    }

    /// マニフェストの突き合わせキーになる識別子を付ける。
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    pub fn path(mut self, path: &str) -> Self {
        self.path = path.to_owned();
        self
    }

    pub fn config(mut self, yaml: &str) -> Self {
        self.config_yaml = Some(yaml.to_owned());
        self
    }

    pub fn target_ruby(mut self, version: &str) -> Self {
        self.target_ruby = version.to_owned();
        self
    }

    pub fn corrected(mut self, corrected: &str) -> Self {
        self.corrected = Some(annotation::dedent(corrected));
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = Some(severity);
        self
    }

    pub fn correctable(mut self, correctable: bool) -> Self {
        self.correctable = Some(correctable);
        self
    }

    pub fn correct_mode(mut self, mode: CorrectMode) -> Self {
        self.correct_mode = mode;
        self
    }

    /// Turns off the guard that refuses a correction leaving the file unparsable (`#41`).
    ///
    /// **Use this when upstream's own output does not parse.** The case then measures the cop --
    /// "is this correction the same as upstream's" -- and stops measuring the engine's separate
    /// decision about whether to write the file at all. **Write the reason in the test**, with
    /// the `ruby -c` verdict on upstream's expected text, so the next reader does not have to
    /// rediscover that upstream is the one breaking it.
    pub fn without_syntax_guard(mut self) -> Self {
        self.skip_syntax_guard = true;
        self
    }

    /// `--only` を使わずに cop を絞る。本家が `--only` と併用を拒む
    /// `Lint/RedundantCopDisableDirective` 専用。
    pub fn without_only(mut self) -> Self {
        self.uses_only = false;
        self
    }

    /// 検査だけ行い、報告をそのまま返す。ハーネスで表現しきれない検証を
    /// テスト側で書きたいときの逃げ道。
    pub fn inspect(&self) -> FileReport {
        let config = self.resolved_config();
        let report =
            engine::inspect_source(&self.path, self.source.clone(), &config, &self.selection())
                .unwrap_or_else(|error| panic!("{}: 検査に失敗した: {error:#}", self.label()));
        self.assert_parsed(&report);
        report
    }

    /// 期待との差分を集める。パニックしないので、既知差分マニフェストと
    /// 突き合わせる側 ([`super::manifest`]) はこちらを使う。
    pub fn verify(&self) -> Verification {
        let report = self.inspect();
        let actual: Vec<Annotation> = report
            .offenses
            .iter()
            .map(|offense| annotation::from_offense(offense, &report.source))
            .collect();
        let mut expected = self.expected.clone().unwrap_or_default();
        annotation::resolve_abbreviations(&mut expected, &actual);

        let mut divergences = Vec::new();
        if self.expected.is_some() {
            divergences.extend(divergence::classify(&expected, &actual));
        }
        divergences.extend(self.uniform_field_divergences(&report));
        divergences.extend(self.correction_divergences());

        Verification {
            rendered_expected: annotation::render(&self.source, &expected),
            rendered_actual: annotation::render(&self.source, &actual),
            cop_names: report
                .offenses
                .iter()
                .map(|offense| offense.cop_name.to_owned())
                .collect(),
            expected,
            actual,
            report,
            divergences,
        }
    }

    /// 期待どおりかを検証する。差分があればパニックする。
    pub fn run(&self) -> FileReport {
        let verification = self.verify();
        assert!(
            verification.divergences.is_empty(),
            "{}",
            self.mismatch_report(&verification)
        );
        verification.report
    }

    pub fn label(&self) -> String {
        match self.id.is_empty() {
            true => self.only.join(","),
            false => self.id.clone(),
        }
    }

    fn selection(&self) -> Selection {
        self.selection_for(false)
    }

    /// `correcting` は本家の `autocorrect?` で、cop がそれ自身で分岐に使う。検査だけの
    /// 実行と `-a` / `-A` の実行では偽と真になるので、offense の突き合わせと autocorrect
    /// の突き合わせは別々の値で検査し直す必要がある。
    fn selection_for(&self, correcting: bool) -> Selection {
        if !self.uses_only {
            // `--only` を渡すと本家は `Lint/RedundantCopDisableDirective` を走らせない。
            // 同じ 1 cop に絞るために、選ばれていない実装済み cop を全部 `--except` する。
            return Selection {
                except: sonicop::rules::rule_names()
                    .filter(|name| !self.selects(name))
                    .map(ToOwned::to_owned)
                    .collect(),
                correcting,
                skip_syntax_guard: self.skip_syntax_guard,
                ..Selection::default()
            };
        }
        Selection {
            only: self.only.clone(),
            correcting,
            skip_syntax_guard: self.skip_syntax_guard,
            ..Selection::default()
        }
    }

    fn resolved_config(&self) -> Arc<Config> {
        resolved_config(&self.config_yaml_text())
    }

    fn config_yaml_text(&self) -> String {
        super::with_target_ruby(self.config_yaml.as_deref(), &self.target_ruby)
    }

    /// `Selection::includes` は `Lint/Syntax` を常に有効にするため、ケースが選んで
    /// いない構文エラーが差分に混ざる。本家が「Error parsing example code」で
    /// 落とすのと同じく、ここで切り分けておく。
    fn assert_parsed(&self, report: &FileReport) {
        if self.only.iter().any(|cop| cop == "Lint/Syntax") {
            return;
        }
        let syntax: Vec<&str> = report
            .offenses
            .iter()
            .filter(|offense| offense.cop_name == "Lint/Syntax")
            .map(|offense| offense.message.as_str())
            .collect();
        assert!(
            syntax.is_empty(),
            "{}: テストソースの構文解析に失敗した:\n  {}\n--- source ---\n{}",
            self.label(),
            syntax.join("\n  "),
            self.source
        );
    }

    fn uniform_field_divergences(&self, report: &FileReport) -> Vec<Divergence> {
        let mut divergences = Vec::new();

        let unexpected: Vec<&str> = report
            .offenses
            .iter()
            .map(|offense| offense.cop_name)
            .filter(|name| !self.selects(name))
            .collect();
        assert!(
            unexpected.is_empty(),
            "{}: 選択していない cop の offense が出た: {}",
            self.label(),
            unexpected.join(", ")
        );

        if let Some(names) = &self.cop_names {
            let mut expected = names.clone();
            expected.sort();
            let mut actual: Vec<String> = report
                .offenses
                .iter()
                .map(|offense| offense.cop_name.to_owned())
                .collect();
            actual.sort();
            if expected != actual {
                divergences.push(Divergence::new(
                    Kind::CopName,
                    expected.join(", "),
                    actual.join(", "),
                ));
            }
        }

        if let Some(locations) = &self.locations {
            let actual: Vec<(usize, usize, usize, usize)> = report
                .offenses
                .iter()
                .map(|offense| {
                    let location = offense.location(&report.source);
                    (
                        location.start_line,
                        location.start_column,
                        location.last_line,
                        location.last_column,
                    )
                })
                .collect();
            if *locations != actual {
                divergences.push(Divergence::new(
                    Kind::Range,
                    format_locations(locations),
                    format_locations(&actual),
                ));
            }
        }

        if let Some(lengths) = &self.lengths {
            let actual: Vec<usize> = report
                .offenses
                .iter()
                .map(|offense| offense.location(&report.source).length)
                .collect();
            if *lengths != actual {
                divergences.push(Divergence::new(
                    Kind::LengthUnit,
                    format_lengths(lengths),
                    format_lengths(&actual),
                ));
            }
        }

        if let Some(severity) = self.severity {
            assert!(
                !report.offenses.is_empty(),
                "{}: severity を検証したが offense が 1 件も無い",
                self.label()
            );
            for offense in &report.offenses {
                if offense.severity != severity {
                    divergences.push(Divergence::new(
                        Kind::Severity,
                        severity.to_string(),
                        offense.severity.to_string(),
                    ));
                }
            }
        }

        if let Some(correctable) = self.correctable {
            assert!(
                !report.offenses.is_empty(),
                "{}: correctable を検証したが offense が 1 件も無い",
                self.label()
            );
            for offense in &report.offenses {
                if offense.correctable != correctable {
                    divergences.push(Divergence::new(
                        Kind::Correctable,
                        correctable.to_string(),
                        offense.correctable.to_string(),
                    ));
                }
            }
        }

        divergences
    }

    fn correction_divergences(&self) -> Vec<Divergence> {
        let Some(expected) = &self.corrected else {
            return Vec::new();
        };
        let config = self.resolved_config();
        // The report handed in came from an inspection that was not correcting, so the cops that
        // branch on `autocorrect?` took the other path. Re-inspect before correcting.
        let selection = self.selection_for(self.correct_mode != CorrectMode::None);
        let report = engine::inspect_source(&self.path, self.source.clone(), &config, &selection)
            .unwrap_or_else(|error| panic!("{}: 検査に失敗した: {error:#}", self.label()));
        let (_, corrected, _) =
            engine::correct_until_stable(report, self.correct_mode, &config, &selection)
                .unwrap_or_else(|error| {
                    panic!("{}: autocorrect が失敗した: {error:#}", self.label())
                });
        match corrected == *expected {
            true => Vec::new(),
            false => vec![Divergence::new(Kind::Correction, expected, &corrected)],
        }
    }

    /// 選択判定は製品コードと同じ [`selector_matches`] に委ねる。ここで別実装を
    /// 持つと、同じ取り違えを両側が共有してテストが素通りしてしまう。
    fn selects(&self, cop_name: &str) -> bool {
        self.only
            .iter()
            .any(|selection| selector_matches(selection, cop_name))
    }

    pub fn mismatch_report(&self, verification: &Verification) -> String {
        let kinds: Vec<String> = verification
            .divergences
            .iter()
            .map(|divergence| divergence.to_string())
            .collect();
        format!(
            "{}: 本家 RuboCop の期待と一致しない\n\
             --- divergences ---\n{}\n\
             --- diff (- expected / + actual) ---\n{}\
             --- expected offenses ---\n{}\
             --- actual offenses ---\n{}",
            self.label(),
            kinds
                .iter()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            diff::unified(
                &verification.rendered_expected,
                &verification.rendered_actual
            ),
            offense_list(&verification.expected, &[]),
            offense_list(&verification.actual, &verification.cop_names),
        ) + &self.guard_hint(verification)
    }

    /// A line pointing at the guard when the correction simply did not happen.
    ///
    /// **`#41` withholds a correction whose result does not parse.** From here that looks
    /// identical to a cop that lost its corrector, and the expected text -- taken from upstream --
    /// looks like the right answer. It is not always: upstream writes text Ruby rejects in a
    /// handful of places, and the guard is what stops us from copying that.
    ///
    /// This cost three people an evening on `Layout/BlockEndNewline` with a heredoc. One line
    /// here is cheaper than the next person rediscovering it.
    fn guard_hint(&self, verification: &Verification) -> String {
        let withheld = verification.divergences.iter().any(|divergence| {
            divergence.kind == Kind::Correction && divergence.sonicop == self.source
        });
        if !withheld {
            return String::new();
        }
        "\n--- ヒント ---\n  \
         **補正が起きず、出力が原本のままです。**安全網 (#41) が発火した可能性があります。\n  \
         期待値が本当に妥当な Ruby かを `ruby -c` で確かめてください。本家が構文エラーを\n  \
         書く箇所があり、そこでは移植版が書き戻しを止めます。\n  \
         cop の訂正だけを測りたいなら `.without_syntax_guard()` を付けてください。\n"
            .to_owned()
    }
}

/// 1 ケースの検証結果。差分の一覧と、失敗時に見せる材料をまとめて持つ。
pub struct Verification {
    pub report: FileReport,
    pub divergences: Vec<Divergence>,
    /// `[...]` を解決したあとの期待。
    pub expected: Vec<Annotation>,
    pub actual: Vec<Annotation>,
    pub cop_names: Vec<String>,
    pub rendered_expected: String,
    pub rendered_actual: String,
}

fn format_locations(locations: &[(usize, usize, usize, usize)]) -> String {
    match locations.is_empty() {
        true => divergence::ABSENT.to_owned(),
        false => locations
            .iter()
            .map(|(start_line, start_column, last_line, last_column)| {
                format!("{start_line}:{start_column}-{last_line}:{last_column}")
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn format_lengths(lengths: &[usize]) -> String {
    match lengths.is_empty() {
        true => divergence::ABSENT.to_owned(),
        false => lengths
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// キャレット注記どおりの offense が出ることを検証する。
pub fn expect_offense(cop: &str, annotated: &str) -> FileReport {
    CopCase::annotated(cop, annotated).run()
}

/// offense が 1 件も出ないことを検証する。本家と同じくソースを注記として
/// 解釈しないので、`^` で始まる行を含むソースも素のまま扱える。
pub fn expect_no_offenses(cop: &str, source: &str) -> FileReport {
    CopCase::new(cop, annotation::dedent(source), Vec::new()).run()
}

/// autocorrect の結果だけを検証する。offense の集合も併せて見たいときは
/// `CopCase::annotated(...).corrected(after).run()` を使う。
pub fn expect_correction(cop: &str, before: &str, after: &str) -> FileReport {
    CopCase::new(cop, annotation::dedent(before), Vec::new())
        .without_offense_check()
        .corrected(after)
        .run()
}

fn offense_list(annotations: &[Annotation], cop_names: &[String]) -> String {
    if annotations.is_empty() {
        return "  (none)\n".to_owned();
    }
    annotations
        .iter()
        .enumerate()
        .map(|(index, annotation)| match cop_names.get(index) {
            Some(cop_name) => format!("  {}. [{cop_name}] {}\n", index + 1, annotation.summary()),
            None => format!("  {}. {}\n", index + 1, annotation.summary()),
        })
        .collect()
}

/// 設定 YAML ごとに `Config` を使い回す。`Config::load` は毎回 config/default.yml
/// (4,000 行超) を YAML から起こすので、ケース数が増えるとここが支配的になる。
static CONFIGS: LazyLock<Mutex<HashMap<String, Arc<Config>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn resolved_config(yaml: &str) -> Arc<Config> {
    if let Some(config) = cache().get(yaml) {
        return Arc::clone(config);
    }

    // 読み込みはロックの外で行う。ケースの YAML が不正でここがパニックしても、
    // 後続のテストが「ロックが壊れている」に化けて真因が隠れないようにする。
    let config = Arc::new(load_config(yaml));
    cache().insert(yaml.to_owned(), Arc::clone(&config));
    config
}

/// キャッシュは失っても検査結果に影響しないので、毒された錠は素通りさせる。
fn cache() -> MutexGuard<'static, HashMap<String, Arc<Config>>> {
    CONFIGS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn load_config(yaml: &str) -> Config {
    // 一時ディレクトリへ設定を書いて読ませる。`Config` は読み込み時に YAML を
    // すべて値へ起こすので、この後ディレクトリが消えても検査には影響しない。
    let directory = TempDir::new().expect("一時ディレクトリを作れなかった");
    let path = directory.path().join(".rubocop.yml");
    std::fs::write(&path, yaml).expect("設定ファイルを書けなかった");
    Config::load(Some(&path), directory.path())
        .unwrap_or_else(|error| panic!("設定を読めなかった: {error:#}\n--- yaml ---\n{yaml}"))
}
