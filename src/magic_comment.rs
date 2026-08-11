//! Ruby magic comments, as RuboCop recognizes them.
//!
//! Mirrors `lib/rubocop/magic_comment.rb`. Several cops need to agree on what counts as a magic
//! comment -- `Layout/EmptyLineAfterMagicComment` looks for the last one before code, and
//! `Style/FrozenStringLiteralComment` looks for one specific setting -- and they disagree in
//! practice unless they share this. The syntax is wider than it first appears: keywords accept both
//! `_` and `-`, matching is case-insensitive, the `#` need not be followed by a space, and editors
//! pack several settings into one comment.

use std::sync::LazyLock;

use regex::Regex;

/// `[[:alnum:]\-_]+`, the token pattern RuboCop borrows from IRB.
const TOKEN: &str = r"[[:alnum:]\-_]+";

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("magic comment pattern must compile")
}

static EMACS: LazyLock<Regex> = LazyLock::new(|| compile(r"-\*-(.+)-\*-"));
static VIM: LazyLock<Regex> = LazyLock::new(|| compile(r"#\s*vim:\s*(.+)"));

static SIMPLE_ENCODING: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"(?i)^\s*#\s*(?:frozen_string_literal:\s*(?:true|false))?\s*(?:en)?coding: ({TOKEN})"
    ))
});
static SIMPLE_FROZEN: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"(?i)^\s*#\s*frozen[_-]string[_-]literal:\s*({TOKEN})\s*$"
    ))
});
static SIMPLE_RBS_INLINE: LazyLock<Regex> =
    LazyLock::new(|| compile(&format!(r"(?i)^\s*#\s*rbs_inline:\s*({TOKEN})\s*$")));
static SIMPLE_SHAREABLE: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"(?i)^\s*#\s*shareable[_-]constant[_-]value:\s*({TOKEN})\s*$"
    ))
});
static SIMPLE_TYPED: LazyLock<Regex> =
    LazyLock::new(|| compile(&format!(r"(?i)^\s*#\s*typed:\s*({TOKEN})\s*$")));

/// One `keyword: value` pair inside an editor comment, such as `coding` in `# -*- coding: utf-8 -*-`.
fn editor_value(payload: &str, separator: char, operator: char, keyword: &Regex) -> Option<String> {
    payload.split(separator).find_map(|token| {
        let token = token.trim();
        let (name, value) = token.split_once(operator)?;
        if !keyword.is_match(name.trim()) {
            return None;
        }
        let value = value.trim();
        // The token pattern is anchored, so a trailing `-*-` or a second setting cannot leak in.
        TOKEN_ONLY
            .is_match(value)
            .then(|| value.to_ascii_lowercase())
    })
}

static TOKEN_ONLY: LazyLock<Regex> = LazyLock::new(|| compile(&format!(r"^{TOKEN}$")));
static KEYWORD_ENCODING: LazyLock<Regex> = LazyLock::new(|| compile(r"(?i)^(?:en)?coding$"));
static KEYWORD_FILEENCODING: LazyLock<Regex> = LazyLock::new(|| compile(r"(?i)^fileencoding$"));
static KEYWORD_FROZEN: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(?i)^frozen[_-]string[_-]literal$"));
static KEYWORD_SHAREABLE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(?i)^shareable[_-]constant[_-]value$"));

/// A comment's text, dispatched to the format it is written in.
pub(crate) enum MagicComment<'a> {
    /// `# -*- coding: utf-8; frozen_string_literal: true -*-`
    Emacs(String),
    /// `# vim: filetype=ruby, fileencoding=ascii-8bit`
    Vim(String),
    /// `# frozen_string_literal: true`, one setting per comment.
    Simple(&'a str),
}

impl<'a> MagicComment<'a> {
    pub(crate) fn parse(comment: &'a str) -> Self {
        if let Some(captures) = EMACS.captures(comment) {
            return Self::Emacs(captures[1].to_owned());
        }
        if let Some(captures) = VIM.captures(comment) {
            return Self::Vim(captures[1].to_owned());
        }
        Self::Simple(comment)
    }

    /// Whether the comment sets anything RuboCop treats as magic. This is what decides which
    /// comments `Layout/EmptyLineAfterMagicComment` must look past.
    pub(crate) fn any(&self) -> bool {
        self.frozen_string_literal().is_some()
            || self.encoding().is_some()
            || self.rbs_inline_valid()
            || self.shareable_constant_value().is_some()
            || self.typed().is_some()
    }

    /// The raw `frozen_string_literal` setting, which is not always `true` or `false` -- a comment
    /// may well say `yes`, and RuboCop reports that as specified but not enabled.
    pub(crate) fn frozen_string_literal(&self) -> Option<String> {
        match self {
            // Vim comments cannot carry this setting.
            Self::Vim(_) => None,
            Self::Emacs(payload) => editor_value(payload, ';', ':', &KEYWORD_FROZEN),
            Self::Simple(comment) => capture(&SIMPLE_FROZEN, comment),
        }
    }

    /// Whether the comment turns the frozen string literal feature on, which requires the value to
    /// be exactly `true` rather than merely truthy.
    pub(crate) fn frozen_string_literal_enabled(&self) -> bool {
        self.frozen_string_literal()
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    }

    /// Whether the comment names the setting at all, whatever it sets it to.
    pub(crate) fn frozen_string_literal_specified(&self) -> bool {
        self.frozen_string_literal().is_some()
    }

    /// Whether the setting actually reaches Ruby. `# frozen_string_literal: yes` names the setting
    /// but does not enable or disable anything, so RuboCop still counts the comment as absent.
    pub(crate) fn valid_literal_value(&self) -> bool {
        self.frozen_string_literal().is_some_and(|value| {
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
        })
    }

    /// The encoding the file declares itself to be in. Ruby's parser reads this before it reads
    /// anything else, which is how a file that is not valid UTF-8 still gets linted.
    pub(crate) fn encoding(&self) -> Option<String> {
        match self {
            Self::Emacs(payload) => editor_value(payload, ';', ':', &KEYWORD_ENCODING),
            Self::Vim(payload) => {
                // A lone `fileencoding` is ignored by Vim itself, so RuboCop needs a second token
                // before it honours one.
                let tokens = payload.split(',').filter(|token| !token.trim().is_empty());
                if tokens.count() < 2 {
                    return None;
                }
                editor_value(payload, ',', '=', &KEYWORD_FILEENCODING)
            }
            Self::Simple(comment) => capture(&SIMPLE_ENCODING, comment),
        }
    }

    fn rbs_inline_valid(&self) -> bool {
        // Only editor-independent comments can carry this, and only two values count.
        let Self::Simple(comment) = self else {
            return false;
        };
        capture(&SIMPLE_RBS_INLINE, comment)
            .is_some_and(|value| matches!(value.as_str(), "enabled" | "disabled"))
    }

    fn shareable_constant_value(&self) -> Option<String> {
        match self {
            Self::Vim(_) => None,
            Self::Emacs(payload) => editor_value(payload, ';', ':', &KEYWORD_SHAREABLE),
            Self::Simple(comment) => capture(&SIMPLE_SHAREABLE, comment),
        }
    }

    fn typed(&self) -> Option<String> {
        match self {
            // Neither editor format can express a Sorbet sigil.
            Self::Emacs(_) | Self::Vim(_) => None,
            Self::Simple(comment) => capture(&SIMPLE_TYPED, comment),
        }
    }
}

fn capture(pattern: &Regex, text: &str) -> Option<String> {
    pattern
        .captures(text)
        .map(|captures| captures[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::MagicComment;

    fn any(comment: &str) -> bool {
        MagicComment::parse(comment).any()
    }

    #[test]
    fn accepts_the_forms_ruby_itself_accepts() {
        // Dashes stand in for underscores and the space after `#` is optional.
        assert!(any("# frozen_string_literal: true"));
        assert!(any("# frozen-string-literal: false"));
        assert!(any("#frozen_string_literal:true"));
        assert!(any("# FROZEN_STRING_LITERAL: TRUE"));
        assert!(any("# encoding: utf-8"));
        assert!(any("#coding: us-ascii"));
        assert!(any("# -*- coding: us-ascii -*-"));
        assert!(any("# -*- frozen_string_literal: true -*-"));
        assert!(any("# typed: strict"));
        assert!(any("# rbs_inline: enabled"));
    }

    #[test]
    fn rejects_comments_that_only_look_magic() {
        assert!(!any("# module to create Makefile for extension modules"));
        assert!(!any("# frozen_string_literal: true is what we want"));
        assert!(!any("# rbs_inline: sometimes"));
        // Vim honours `fileencoding` only alongside another token.
        assert!(!any("# vim: fileencoding=ascii-8bit"));
        assert!(any("# vim: filetype=ruby, fileencoding=ascii-8bit"));
    }

    #[test]
    fn reports_the_literal_setting_rather_than_its_truthiness() {
        let comment = MagicComment::parse("# frozen_string_literal: yes");
        assert_eq!(comment.frozen_string_literal().as_deref(), Some("yes"));
        assert!(!comment.frozen_string_literal_enabled());
        assert!(
            MagicComment::parse("# frozen-string-literal: TRUE").frozen_string_literal_enabled()
        );
    }
}
