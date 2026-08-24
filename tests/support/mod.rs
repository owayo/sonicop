//! テスト共有のハーネス。`tests/cops.rs` (in-process) と `tests/cli.rs`
//! (サブプロセス) の両方から使う。
//!
//! # 構成
//!
//! - [`annotation`] — 本家 `expect_offense` のキャレット注記の読み書き。
//!   `parse` が注記付きソースを「素のソース + 注記一覧」に割り、`render` が
//!   その逆を行う。offense → 注記の変換 (`from_offense`) もここ。
//! - [`case`] — 本家 spec 1 ケース分の中間表現 [`case::CopCase`] と、それを
//!   `engine::inspect_source` に当てて検証する層。注記パーサは `CopCase` を
//!   組み立てる入口の 1 つに過ぎず、機械変換は [`case::CopCase::new`] を直接叩く。
//! - [`diff`] — 失敗時の行単位差分。
//! - [`project`] — CLI テスト用の一時プロジェクトと JSON の構造比較。
//!
//! # 使い方
//!
//! ```ignore
//! expect_offense("Style/RedundantReturn", r#"
//!     def foo
//!       return 1
//!       ^^^^^^ Redundant `return` detected.
//!     end
//! "#);
//! expect_no_offenses("Style/RedundantReturn", "def foo\n  1\nend\n");
//! expect_correction("Style/RedundantReturn", before, after);
//!
//! CopCase::annotated("Style/StringLiterals", annotated)
//!     .config("Style/StringLiterals:\n  EnforcedStyle: double_quotes\n")
//!     .target_ruby("3.1")
//!     .corrected(after)
//!     .run();
//! ```
//!
//! offense は**集合として完全一致**を要求する。余分な検出も不足も落ちる。
//! 比較の軸は cop 名 / 行 / カラム / 先頭行のレンジ長 / メッセージで、
//! severity・correctable・複数行レンジの終端は必要なケースだけ上乗せする
//! ([`case::CopCase::severity`] / [`case::CopCase::correctable`] /
//! [`case::CopCase::locations`])。
//!
//! テスト対象ごとに使う部分が違うので、片側で未使用になる項目を許す。
#![allow(dead_code)]

use serde_yaml_ng::{Mapping, Value};

pub mod annotation;
pub mod case;
pub mod diff;
pub mod divergence;
pub mod manifest;
pub mod project;

/// `.rubocop.yml` 相当の YAML へ `AllCops/TargetRubyVersion` を差し込む。
///
/// 明示しないと sonicop は gemspec の `required_ruby_version` / `.ruby-version` /
/// `.tool-versions` を上へ辿って拾う。テストを実行した場所によって対象 Ruby が
/// 変わると同じ入力で結果が割れるので、テストは必ず版を固定する。
///
/// 文字列連結ではなくマッピングとして合成するので、呼び出し側が `AllCops` を
/// 持っていてもキーが重複しない。
pub fn with_target_ruby(yaml: Option<&str>, version: &str) -> String {
    with_target_ruby_enabling(yaml, version, &[])
}

/// 同じことを、`--only` が選ぶ cop に `Enabled: true` を書き足したうえで行う。
///
/// **記録の再現には記録の条件が要る。**`spec_fixture_gen` は `--only` の cop に
/// `Enabled: true` を必ず書く。既定で無効な cop -- `Style/DisableCopsWithinSourceCodeDirective`
/// はその一つ -- は、この 1 行があるかどうかで挙動そのものが変わる (`prevent_directive_disabling?`
/// は `Enabled` が明示的に true のときだけ効く)。書かずに走らせると、cop の不具合と
/// 見分けのつかない差分が出る。
pub fn with_target_ruby_enabling(yaml: Option<&str>, version: &str, only: &[String]) -> String {
    let mut root: Value = match yaml {
        Some(yaml) if !yaml.trim().is_empty() => serde_yaml_ng::from_str(yaml)
            .unwrap_or_else(|error| panic!("設定 YAML が不正: {error}\n--- yaml ---\n{yaml}")),
        _ => Value::Mapping(Mapping::new()),
    };
    let mapping = root
        .as_mapping_mut()
        .expect("設定 YAML はマッピングであること");
    let all_cops = mapping
        .entry(Value::String("AllCops".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    all_cops
        .as_mapping_mut()
        .expect("AllCops はマッピングであること")
        .insert(
            Value::String("TargetRubyVersion".to_owned()),
            Value::String(version.to_owned()),
        );
    for cop in only {
        let entry = mapping
            .entry(Value::String(cop.clone()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if let Some(section) = entry.as_mapping_mut() {
            section
                .entry(Value::String("Enabled".to_owned()))
                .or_insert(Value::Bool(true));
        }
    }
    serde_yaml_ng::to_string(&root).expect("合成した設定 YAML を書き出せなかった")
}
