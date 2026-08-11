//! 本家 RuboCop との差分を機械可読な形で表す。
//!
//! 分類は実コーパスでの A/B 検証で使っているもの (false negative / false positive /
//! message / range / severity) に揃えてある。両方の結果を同じ軸で合算できる。

use std::fmt;

use super::annotation::Annotation;

/// 差分の種類。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    /// 本家は検出するが sonicop は検出しない。
    FalseNegative,
    /// sonicop だけが検出する。
    FalsePositive,
    /// 位置は合っているがメッセージが違う。
    Message,
    /// メッセージは合っているが位置か長さが違う。
    Range,
    Severity,
    Correctable,
    /// `location.length` の単位 (本家は文字数、sonicop はバイト数)。
    LengthUnit,
    /// autocorrect の結果。
    Correction,
    /// offense が属する cop 名。
    CopName,
}

impl Kind {
    pub const ALL: &'static [Self] = &[
        Self::FalseNegative,
        Self::FalsePositive,
        Self::Message,
        Self::Range,
        Self::Severity,
        Self::Correctable,
        Self::LengthUnit,
        Self::Correction,
        Self::CopName,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FalseNegative => "false_negative",
            Self::FalsePositive => "false_positive",
            Self::Message => "message",
            Self::Range => "range",
            Self::Severity => "severity",
            Self::Correctable => "correctable",
            Self::LengthUnit => "length_unit",
            Self::Correction => "correction",
            Self::CopName => "cop_name",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == value)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 差分 1 件。`upstream` / `sonicop` はマニフェストと文字列で突き合わせるので、
/// 同じ差分なら必ず同じ表現になるように組み立てる。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Divergence {
    pub kind: Kind,
    pub upstream: String,
    pub sonicop: String,
}

impl Divergence {
    pub fn new(kind: Kind, upstream: impl Into<String>, sonicop: impl Into<String>) -> Self {
        Self {
            kind,
            upstream: upstream.into(),
            sonicop: sonicop.into(),
        }
    }
}

impl fmt::Display for Divergence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "[{}] 本家: {} / sonicop: {}",
            self.kind, self.upstream, self.sonicop
        )
    }
}

/// 検出なしを表す印。マニフェストにもこの文字列で載る。
pub const ABSENT: &str = "(検出なし)";

/// 期待 (本家) と実際 (sonicop) の offense 集合を突き合わせて差分に分類する。
///
/// 素朴に「件数が違う」で片付けず、同じ offense が message だけ違うのか位置だけ
/// 違うのかを切り分ける。修正の優先順位付けが分類の粒度に依存するため。
pub fn classify(expected: &[Annotation], actual: &[Annotation]) -> Vec<Divergence> {
    let mut pending_expected: Vec<&Annotation> = expected.iter().collect();
    let mut pending_actual: Vec<&Annotation> = actual.iter().collect();
    let mut divergences = Vec::new();

    // 1. 完全一致は取り除く。
    pending_expected.retain(|annotation| {
        match pending_actual
            .iter()
            .position(|other| *other == *annotation)
        {
            Some(index) => {
                pending_actual.remove(index);
                false
            }
            None => true,
        }
    });

    // 2. 位置が同じものは message (と長さ) の違い。
    let mut unresolved = Vec::new();
    for annotation in pending_expected.drain(..) {
        let found = pending_actual
            .iter()
            .position(|other| (other.line, other.column) == (annotation.line, annotation.column));
        match found {
            Some(index) => {
                let other = pending_actual.remove(index);
                if other.message != annotation.message {
                    divergences.push(Divergence::new(
                        Kind::Message,
                        &annotation.message,
                        &other.message,
                    ));
                }
                if other.length != annotation.length {
                    divergences.push(Divergence::new(
                        Kind::Range,
                        annotation.span(),
                        other.span(),
                    ));
                }
            }
            None => unresolved.push(annotation),
        }
    }

    // 3. メッセージが同じものは位置の違い。
    for annotation in unresolved {
        let found = pending_actual
            .iter()
            .position(|other| other.message == annotation.message);
        match found {
            Some(index) => {
                let other = pending_actual.remove(index);
                divergences.push(Divergence::new(
                    Kind::Range,
                    annotation.span(),
                    other.span(),
                ));
            }
            None => divergences.push(Divergence::new(
                Kind::FalseNegative,
                annotation.summary(),
                ABSENT,
            )),
        }
    }

    // 4. 残った実際の offense は sonicop だけが出しているもの。
    for annotation in pending_actual {
        divergences.push(Divergence::new(
            Kind::FalsePositive,
            ABSENT,
            annotation.summary(),
        ));
    }

    divergences.sort();
    divergences
}
