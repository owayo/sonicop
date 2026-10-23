#[path = "syntax_legacy_targets.rs"]
mod legacy_targets;

use crate::diagnostic::Offense;
use crate::legacy_parser::{self, Version};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

/// 旧版は parser gem の回復状態、新版は Prism の診断をそのまま使う。
/// tree-sitter の ERROR に限ると、本家だけが拒否する入力を取りこぼす。
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let target = context.target_ruby_version();
    // 2.0〜2.6 は本家 1.89.0 の対象外だが、既存 API が許すので従来の判定を保つ。
    if target < RubyVersion::new(2, 7) {
        legacy_targets::check(context, offenses);
        return;
    }
    let mut diagnostics: Vec<(std::ops::Range<usize>, String)> =
        if let Some(found) = crate::prism::diagnostics(context) {
            found
                .into_iter()
                .map(|diagnostic| (diagnostic.range, diagnostic.reason))
                .collect()
        } else {
            let engine = context
                .setting_of::<String>("AllCops", "ParserEngine")
                .unwrap_or_default();
            let Some(version) = Version::for_target(target, engine == "parser_whitequark") else {
                // 本家が持たない対象版と engine の組も、既存 API の挙動を維持する。
                legacy_targets::check(context, offenses);
                return;
            };
            legacy_parser::diagnostics(context.source.text(), version)
                .into_iter()
                .map(|diagnostic| (diagnostic.range, diagnostic.message))
                .collect()
        };
    // Base#add_offense は表示用に広げた範囲をキーにし、同じ範囲の最初の診断を残す。
    // 安定ソートで、同じ範囲についての発生順を保持する。
    diagnostics.sort_by_key(|(range, _)| (range.start, range.end));
    diagnostics.dedup_by(|left, right| left.0 == right.0);
    for (range, reason) in diagnostics {
        offenses.push(context.offense(syntax_message(&reason, target), range));
    }
}

fn syntax_message(reason: &str, target: RubyVersion) -> String {
    format!(
        "{reason}\n(Using Ruby {target} parser; configure using `TargetRubyVersion` parameter, under `AllCops`)"
    )
}
