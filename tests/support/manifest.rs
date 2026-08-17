//! 既知差分マニフェスト。本家 RuboCop との差分を**データとして 1 箇所に**持ち、
//! ケースの実行結果と突き合わせる。
//!
//! `#[ignore]` で差分を退避すると、**直っても誰も気づかず陳腐化した ignore が
//! 永遠に残る**。マニフェスト方式は「直ったのにエントリが残っている」も失敗に
//! するので、修正がマニフェストの掃除を強制する。
//!
//! | 実際 | マニフェスト | 判定 |
//! |---|---|---|
//! | 一致 | エントリなし | pass |
//! | 差分あり | 同じ差分のエントリあり | pass (既知) |
//! | 差分あり | エントリなし | **FAIL** — 新しい退行 |
//! | 一致 | エントリあり | **FAIL** — 直ったので消すべき |
//! | 一致 | エントリあり + **ケースが移植版の側** | **FAIL** — ★ 向きが逆。消してはいけない |
//!
//! ★ 最後の 1 行が要る理由。上から 4 行目の判定は正しいが、**「一致」になる理由が 2 つある**。
//! 本当に直った場合と、ケースに移植版の出力を書いてしまった場合である。後者は差分が
//! 0 件になるので 4 行目に落ち、「消してください」と表示される。**間違いの一方向だけが
//! 作業指示の形で出てくる**ので、素直に従うほど登録が減る。2026-08-17 に実際に踏んだ。
//!
//! 判別は [`reverses_upstream_and_sonicop`] が行う。**ただし `kind: correction` だけ** --
//! 射程はその関数の doc を読むこと。
//!
//! 書式は YAML。`toml` クレートを増やさずに済み、`.rubocop_todo.yml` と同じ
//! 「既知の未対応を溜める入れ物」という位置づけにも合う。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::case::CopCase;
use super::divergence::{Divergence, Kind};

/// マニフェストの既定の置き場。
pub const DEFAULT_PATH: &str = "tests/conformance/known_divergences.yml";

/// 差分エントリ 1 件。`cop` / `case` / `kind` / `upstream` / `sonicop` の 5 つ組が
/// 突き合わせキーで、`note` は人間向けの説明。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    pub cop: String,
    #[serde(rename = "case")]
    pub case_id: String,
    pub kind: String,
    pub upstream: String,
    pub sonicop: String,
    #[serde(default)]
    pub note: String,
}

impl Entry {
    fn from_divergence(case: &CopCase, divergence: &Divergence, note: &str) -> Self {
        Self {
            cop: case.only.join(","),
            case_id: case.label(),
            kind: divergence.kind.to_string(),
            upstream: divergence.upstream.clone(),
            sonicop: divergence.sonicop.clone(),
            note: note.to_owned(),
        }
    }
}

/// 検証コーパスが**見ていないもの**。レポートに必ず出す。
///
/// 「本家ソース 1,743 ファイルで 100% 一致」のような報告はコーパス選択の産物で
/// あり得る。何を検証していないかを書かないレポートは、次に読む人を同じ罠に
/// 落とすため、マニフェストの一部として管理する。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlindSpot {
    pub corpus: String,
    pub not_covered: String,
    pub consequence: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub blind_spots: Vec<BlindSpot>,
    #[serde(default)]
    pub divergences: Vec<Entry>,
}

/// ケース 1 件の判定結果。
#[derive(Debug, Default)]
pub struct Verdict {
    /// マニフェストに無い差分。新しい退行。
    pub unknown: Vec<Divergence>,
    /// マニフェストにあるが、もう出ていない差分。**直ったとは限らない** --
    /// [`Verdict::reversed`] を先に見ること。
    pub resolved: Vec<Entry>,
    /// 差分が出ていない理由が「直った」ではなく **ケースが移植版の側を写している** もの。
    ///
    /// ケースは本家の振る舞いを持ち、マニフェストが移植版の外れを持つ、という分担になって
    /// いる。ケースに移植版の出力を書くと差分が 0 件になり、登録が `resolved` に落ちて
    /// 「消してください」と表示される。**間違いの一方向だけが作業指示の形で出てくる**ので、
    /// 素直に従うほど登録が減る。ここはその形だけを取り分ける。
    pub reversed: Vec<Entry>,
    /// マニフェストどおりの既知差分。
    pub known: Vec<Entry>,
}

impl Verdict {
    pub fn is_clean(&self) -> bool {
        self.unknown.is_empty() && self.resolved.is_empty() && self.reversed.is_empty()
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Self {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} を読めない: {error}", path.display()));
        serde_yaml_ng::from_str(&text)
            .unwrap_or_else(|error| panic!("{} が不正: {error}", path.display()))
    }

    /// リポジトリルート起点でマニフェストを読む。テストの cwd はクレートルート。
    pub fn load_default() -> Self {
        Self::load(&PathBuf::from(DEFAULT_PATH))
    }

    fn entries_for(&self, case_id: &str) -> Vec<&Entry> {
        self.divergences
            .iter()
            .filter(|entry| entry.case_id == case_id)
            .collect()
    }

    /// ケースを実行し、差分をマニフェストと突き合わせる。
    pub fn judge(&self, case: &CopCase) -> (Verdict, String) {
        let verification = case.verify();
        let case_id = case.label();
        let recorded = self.entries_for(&case_id);

        let mut verdict = Verdict::default();
        let mut matched = vec![false; recorded.len()];
        for divergence in &verification.divergences {
            let found = recorded.iter().enumerate().position(|(index, entry)| {
                !matched[index]
                    && entry.kind == divergence.kind.to_string()
                    && entry.upstream == divergence.upstream
                    && entry.sonicop == divergence.sonicop
            });
            match found {
                Some(index) => {
                    matched[index] = true;
                    verdict.known.push(recorded[index].clone());
                }
                None => verdict.unknown.push(divergence.clone()),
            }
        }
        for (index, entry) in recorded.iter().enumerate() {
            if !matched[index] {
                match reverses_upstream_and_sonicop(case, entry) {
                    true => verdict.reversed.push((*entry).clone()),
                    false => verdict.resolved.push((*entry).clone()),
                }
            }
        }

        let detail = match verdict.is_clean() {
            true => String::new(),
            false => case.mismatch_report(&verification),
        };
        (verdict, detail)
    }

    /// マニフェスト自体の健全性。未知の種別・説明の欠落・消えたケースへの参照を落とす。
    ///
    /// 同じ 5 つ組のエントリが複数あることは正当 (1 ケース内の複数 offense が同じ
    /// 差分を起こす場合)。取り違えた重複は「登録があるのに出ない」判定が拾う。
    pub fn problems(&self, known_case_ids: &[String]) -> Vec<String> {
        let mut problems = Vec::new();
        for entry in &self.divergences {
            if Kind::parse(&entry.kind).is_none() {
                problems.push(format!(
                    "{}: 未知の kind `{}` (使えるのは {})",
                    entry.case_id,
                    entry.kind,
                    Kind::ALL
                        .iter()
                        .map(|kind| kind.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                ));
            }
            if entry.note.trim().is_empty() {
                problems.push(format!(
                    "{} [{}]: note が空。差分はバグの記録なので一行の説明を書くこと",
                    entry.case_id, entry.kind
                ));
            }
            if !known_case_ids.contains(&entry.case_id) {
                problems.push(format!(
                    "{}: そんなケースは無い。ケースを消したならエントリも消すこと",
                    entry.case_id
                ));
            }
        }
        problems
    }
}

/// 登録が「出なくなった」理由が、直ったからではなく **ケースが移植版の側を写しているから**
/// ではないか。
///
/// ケースは本家の振る舞いを持ち、マニフェストが移植版の外れを持つ。ケースに移植版の出力を
/// 書くと差分が 0 件になり、`resolved` として「消してください」と表示される。**間違いの
/// 一方向だけが作業指示の形で出てくる**ので、素直に従うほど登録が減る。
///
/// 判別子はエントリ自身が持っている。差分が出ていない = 移植版の出力はケースの期待値と
/// 等しい。だから:
///
/// ```text
/// ケースの期待値 == entry.sonicop   → 向きが逆。登録はまだ生きている
/// ケースの期待値 == entry.upstream  → 本当に直った。消してよい
/// どちらとも違う                    → 3 つ目。人が読む
/// ```
///
/// # 射程 (★ ここが全部ではない)
///
/// **`kind: correction` だけを機械で判定する。**その kind のケース側の期待値は
/// [`CopCase::corrected`] という 1 つの文字列なので、完全一致で比べられるため。
///
/// `false_negative` / `false_positive` / `message` / `range` などの offense 系は、
/// `Divergence` 側が整形済みの 1 行を持つのに対しケース側は `Annotation` の列なので、
/// 同じ整形を通さないと比べられない。**整形を揃え損ねた判別子は、常に「どちらとも違う」を
/// 返して素通りする** -- 検査が無いより悪い (通ると「確認済み」に見える)。だからそれらは
/// 判定せず、`resolved` の失敗文に 2 つの仮説を両方書いて人に渡す。
///
/// 2 例目が実際に出たときに、その実例を対照にして整形の共有化をやる。**起きていない事故に
/// 対して先に作らない。**
fn reverses_upstream_and_sonicop(case: &CopCase, entry: &Entry) -> bool {
    if entry.kind != Kind::Correction.as_str() {
        return false;
    }
    case.corrected
        .as_deref()
        .is_some_and(|expected| expected == entry.sonicop)
}
/// 実行結果から差分エントリの YAML を組み立てる。
///
/// **マニフェストを自動で書き換えることは意図的にしていない。**差分はバグの
/// 記録なので、増えるときは人間が中身を見るべきだから。出力を見て、説明を
/// 付けてから手で取り込む。
pub fn suggest(case: &CopCase, divergences: &[Divergence]) -> String {
    let entries: Vec<Entry> = divergences
        .iter()
        .map(|divergence| Entry::from_divergence(case, divergence, "TODO: 差分の説明を書く"))
        .collect();
    serde_yaml_ng::to_string(&entries).unwrap_or_else(|error| format!("# 書き出せない: {error}"))
}

#[cfg(test)]
mod reversal_guard {
    use super::*;

    fn entry(kind: &str, upstream: &str, sonicop: &str) -> Entry {
        Entry {
            cop: "Style/EmptyLiteral".to_owned(),
            case_id: "probe".to_owned(),
            kind: kind.to_owned(),
            upstream: upstream.to_owned(),
            sonicop: sonicop.to_owned(),
            note: "probe".to_owned(),
        }
    }

    fn case_expecting(corrected: &str) -> CopCase {
        CopCase::new("Style/EmptyLiteral", "recv&.foo Hash.new\n", Vec::new()).corrected(corrected)
    }

    /// **陽性対照。**これが落ちるときは番人が死んでいる。
    ///
    /// 番人は「登録を消すな」としか言わないので、**死んでも誰も困らない**。困らないまま
    /// 通り続けるのが最悪なので、発火することを別に確かめる。
    #[test]
    fn a_case_written_from_the_sonicop_side_is_caught() {
        let entry = entry("correction", "recv&.foo {}\n", "recv&.foo({})\n");
        assert!(reverses_upstream_and_sonicop(
            &case_expecting("recv&.foo({})\n"),
            &entry
        ));
    }

    /// 陰性対照 1: ケースが本家の側を写していれば、それは本当に直った側である。
    #[test]
    fn a_case_written_from_the_upstream_side_is_not_caught() {
        let entry = entry("correction", "recv&.foo {}\n", "recv&.foo({})\n");
        assert!(!reverses_upstream_and_sonicop(
            &case_expecting("recv&.foo {}\n"),
            &entry
        ));
    }

    /// 陰性対照 2: どちらとも違えば 3 つ目の枝で、人が読む。
    #[test]
    fn a_case_matching_neither_side_is_not_caught() {
        let entry = entry("correction", "recv&.foo {}\n", "recv&.foo({})\n");
        assert!(!reverses_upstream_and_sonicop(
            &case_expecting("recv&.foo(Hash.new)\n"),
            &entry
        ));
    }

    /// ★ **射程の対照。**`correction` 以外は判定しないので、向きが逆でも偽を返す。
    ///
    /// これが真になったら射程が広がっているので、`resolved` の失敗文にある
    /// 「自動判定は kind: correction のみ」も一緒に直すこと。**この対照は、
    /// 番人の射程と文面がずれることを止めるためにある。**
    #[test]
    fn other_kinds_are_deliberately_not_judged() {
        let entry = entry("false_positive", "(検出なし)", "recv&.foo({})\n");
        assert!(!reverses_upstream_and_sonicop(
            &case_expecting("recv&.foo({})\n"),
            &entry
        ));
    }
}
