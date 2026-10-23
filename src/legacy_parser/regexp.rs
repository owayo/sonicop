//! builder の `static_regexp` (`Regexp.new(source, options)`) のうち、診断と静的環境に効くところ。
//!
//! parser gem は補間の無い正規表現リテラルを実行中の Ruby (Onigmo) でコンパイルし、失敗すれば
//! その例外の文言を `invalid_regexp` の診断にする。`=~` の左辺なら名前付きキャプチャを
//! ローカル変数として宣言する。

/// 名前付きキャプチャの名前 (`Regexp#names`: 出現順、重複なし)。
pub(crate) fn names(source: &str, flags: &str) -> Vec<String> {
    crate::regexp_names::named_groups(source, flags.contains('x'))
}

/// 正規表現のコンパイル診断は従来のバックエンドも未対応。
/// 捕捉名の字句分類とは別に Onigmo の検証が必要なので、受理の証明として使わない。
pub(crate) fn validate(source: &str, flags: &str) -> Result<(), String> {
    let _ = (source, flags);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn named_captures_are_read_outside_classes_and_escapes() {
        assert_eq!(
            names("(?<year>\\d+)-(?'month'\\d+)", ""),
            vec!["year", "month"]
        );
        assert_eq!(
            names("(?<=a)(?<!b)\\(?<no>x)[(?<no2>)]", ""),
            Vec::<String>::new()
        );
        assert_eq!(names("(?<a>x)(?<a>y)", ""), vec!["a"]);
        assert_eq!(names("# (?<c>x)\n(?<d>y)", "x"), vec!["d"]);
    }

    #[test]
    fn comment_and_scoped_options_do_not_declare_phantom_variables() {
        assert!(names("(?#(?<foo>)", "").is_empty());
        assert!(names("(?x)#(?<foo>x)\n", "").is_empty());
        assert_eq!(names("(?x:#(?<no>x)\n)(?<yes>a)", ""), vec!["yes"]);
        assert_eq!(names("(?<one>(?<two>x))(?<one>y)", ""), vec!["one", "two"]);
        for source in [
            "/(?#(?<foo>)/ =~ s\nfoo /bar/\n",
            "/(?x)#(?<foo>x)\n/ =~ s\nfoo /bar/\n",
        ] {
            for version in [
                super::super::Version::Ruby27,
                super::super::Version::Ruby30,
                super::super::Version::Ruby31,
                super::super::Version::Ruby32,
                super::super::Version::Ruby33,
                super::super::Version::Ruby34,
            ] {
                assert!(
                    super::super::diagnostics(source, version).is_empty(),
                    "{source:?} {version:?}"
                );
            }
        }
    }
}
