//! parser gem から読み出した racc と Ragel の表。中身は `scripts/legacy_parser/generate.rb` の生成物で、
//! ここには生成物を引くための型だけを置く。

// 生成表は文法全体を保持する。駆動器が直接読まない状態と優先順位トークンも削らない。
#![allow(dead_code, clippy::all)]

pub(crate) mod actions;
pub(crate) mod lexer;
pub(crate) mod lexer_exec;
pub(crate) mod lexer_strings;
pub(crate) mod ruby27;
pub(crate) mod ruby30;
pub(crate) mod ruby31;
pub(crate) mod ruby32;
pub(crate) mod ruby33;
pub(crate) mod ruby34;
pub(crate) mod tokens;

/// racc の表で `nil` だった位置。
pub(crate) const NONE: i16 = i16::MIN;

/// racc の還元表の 1 行 (`racc_reduce_table` の 3 つ組)。
pub(crate) struct Rule {
    /// 右辺の記号の数。埋め込み動作の規則は 0 で、cparse はその動作に空の `val` を渡す。
    pub(crate) len: u8,
    /// 左辺の非終端記号の番号 (`nt_base` 以上)。
    pub(crate) lhs: u16,
    pub(crate) action: actions::Action,
}

/// `Racc_arg` と終端記号の名前表。
pub(crate) struct Grammar {
    pub(crate) action_table: &'static [i16],
    pub(crate) action_check: &'static [i16],
    pub(crate) action_default: &'static [i16],
    pub(crate) action_pointer: &'static [i16],
    pub(crate) goto_table: &'static [i16],
    pub(crate) goto_check: &'static [i16],
    pub(crate) goto_default: &'static [i16],
    pub(crate) goto_pointer: &'static [i16],
    pub(crate) nt_base: i16,
    pub(crate) shift_n: i16,
    pub(crate) reduce_n: i16,
    pub(crate) rules: &'static [Rule],
    /// `tokens::Tok` ごとの終端記号の番号。この文法に無いトークンは 1 (`error`)。
    pub(crate) token_ids: &'static [u16],
    /// 終端記号の番号ごとの名前 (`Racc_token_to_s_table`)。`unexpected token` の診断はこれを引く。
    pub(crate) names: &'static [&'static str],
}

/// 表の 1 項目。範囲外と `nil` はどちらも「無い」。
pub(crate) fn entry(table: &[i16], index: i32) -> Option<i16> {
    usize::try_from(index)
        .ok()
        .and_then(|index| table.get(index))
        .copied()
        .filter(|&value| value != NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gem の表をそのまま写したことの確認。`Racc_arg` の大きさと、`shift_n`・`reduce_n` を突き合わせる。
    #[test]
    fn the_tables_keep_the_shape_racc_generated() {
        for (grammar, states, rules) in [
            (&ruby27::GRAMMAR, 1194, 716),
            (&ruby30::GRAMMAR, 1219, 729),
            (&ruby31::GRAMMAR, 1238, 745),
            (&ruby32::GRAMMAR, 1234, 741),
            (&ruby33::GRAMMAR, 1250, 742),
            (&ruby34::GRAMMAR, 1250, 742),
        ] {
            assert_eq!(grammar.shift_n, states);
            assert_eq!(grammar.reduce_n, rules);
            assert_eq!(grammar.rules.len(), rules as usize);
            assert_eq!(grammar.action_default.len(), states as usize);
            assert_eq!(grammar.action_table.len(), grammar.action_check.len());
            assert_eq!(grammar.goto_table.len(), grammar.goto_check.len());
            assert_eq!(grammar.token_ids.len(), tokens::Tok::COUNT);
            assert_eq!(grammar.names.len(), grammar.nt_base as usize);
            assert_eq!(grammar.names[0], "$end");
            assert_eq!(grammar.names[1], "error");
        }
    }
}
