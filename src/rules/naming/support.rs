//! Identifier spelling shared by the cops that enforce `EnforcedStyle`.

use std::sync::LazyLock;

use regex::Regex;

static SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z_][a-zA-Z0-9_]*[!?=]?$").unwrap());
static CAMEL_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-zA-Z0-9]*[!?=]?$").unwrap());

pub(super) fn valid_name(name: &str, style: &str) -> bool {
    if style == "camelCase" {
        CAMEL_CASE.is_match(name) && !name.contains('_')
    } else {
        SNAKE_CASE.is_match(name)
            && !name
                .trim_matches(['?', '!', '='])
                .chars()
                .any(char::is_uppercase)
    }
}
