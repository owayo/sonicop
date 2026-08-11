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
    /// マニフェストにあるが、もう出ていない差分。直ったので消すべき。
    pub resolved: Vec<Entry>,
    /// マニフェストどおりの既知差分。
    pub known: Vec<Entry>,
}

impl Verdict {
    pub fn is_clean(&self) -> bool {
        self.unknown.is_empty() && self.resolved.is_empty()
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
                verdict.resolved.push((*entry).clone());
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
