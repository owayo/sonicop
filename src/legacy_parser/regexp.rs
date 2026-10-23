//! builder の `static_regexp` (`Regexp.new(source, options)`) のうち、診断と静的環境に効くところ。
//!
//! parser gem は補間の無い正規表現リテラルを実行中の Ruby (Onigmo) でコンパイルし、失敗すれば
//! その例外の文言を `invalid_regexp` の診断にする。`=~` の左辺なら名前付きキャプチャを
//! ローカル変数として宣言する。

/// Regexp#names は、ソースの識別子と同じ Symbol になる名前だけが参照できる。
pub(crate) fn names(
    source: &[u8],
    encoding: crate::engine::LiteralEncoding,
    flags: &str,
    identifier_encoding: crate::engine::LiteralEncoding,
) -> Option<Vec<String>> {
    crate::regexp_validation::named_groups(source, encoding, flags, identifier_encoding)
}

/// Regexp.new の符号化変換・前処理・Onigmo コンパイルを共通層へ渡す。
pub(crate) fn validate(
    source: &[u8],
    encoding: crate::engine::LiteralEncoding,
    flags: &str,
) -> Result<(), String> {
    crate::regexp_validation::validate(source, encoding, flags)
}

#[cfg(test)]
mod tests {
    fn names(source: &str, flags: &str) -> Vec<String> {
        let encoding = crate::engine::LiteralEncoding::Text;
        super::names(source.as_bytes(), encoding, flags, encoding).unwrap_or_default()
    }

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
