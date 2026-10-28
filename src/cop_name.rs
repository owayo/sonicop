//! How a cop's qualified name decomposes, and when a user-supplied selector names it.
//!
//! RuboCop settles both questions in `Cop::Badge` and `Cop::Registry`. Keeping them in one place
//! here is what stops `--only`, `# rubocop:disable`, configuration inheritance and `--show-cops`
//! from disagreeing about a nested cop such as `Chef/Correctness/ServiceResource`.

/// Everything before the last `/`, or the whole name when it has none.
///
/// `Badge#initialize` joins every segment but the last, so the department of
/// `Chef/Correctness/ServiceResource` is `Chef/Correctness` rather than `Chef`.
pub fn department(cop_name: &str) -> &str {
    cop_name
        .rsplit_once('/')
        .map_or(cop_name, |(department, _)| department)
}

/// The cop's department and every namespace enclosing it, the department itself first.
///
/// Only plugin ownership widens like this: a gem that declares `I18n` also ships the nested
/// `I18n/GetText` department. Selectors and configuration lookups must keep using [`department`],
/// which stops at the one department the cop actually belongs to.
pub fn department_ancestors(cop_name: &str) -> impl Iterator<Item = &str> {
    let department = department(cop_name);
    std::iter::once(department).chain(
        department
            .match_indices('/')
            .map(move |(offset, _)| &department[..offset]),
    )
}

/// Whether a selector the user wrote -- in `--only`, `--except`, or a `rubocop:disable` comment --
/// names this cop.
///
/// `Badge#match_name?` compares the qualified name and the department, both in full, so an outer
/// namespace does not reach a nested cop: `Chef` leaves `Chef/Correctness/ServiceResource` alone
/// and only `Chef/Correctness` selects it.
pub fn selector_matches(selector: &str, cop_name: &str) -> bool {
    selector == cop_name || selector == department(cop_name)
}

/// Badge.parse と同じ表記変換を、CLI と設定文書の名前解決で共有する。
pub(crate) fn parse_badge(name: &str) -> (String, bool) {
    let mut parts: Vec<&str> = name.split('/').collect();
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    let qualified = parts.len() > 1;
    let parts: Vec<String> = parts.into_iter().map(badge_part).collect();
    (parts.join("/"), qualified)
}

fn badge_part(part: &str) -> String {
    if part == "rspec" {
        return "RSpec".to_owned();
    }
    // Badge.camel_case の /^[a-z]|_[a-z]/ は空白を消さず、ASCII 小文字だけを変える。
    let mut result = String::with_capacity(part.len());
    let mut characters = part.chars().peekable();
    let mut line_start = true;
    while let Some(character) = characters.next() {
        if character == '_' && characters.peek().is_some_and(char::is_ascii_lowercase) {
            result.push(characters.next().unwrap().to_ascii_uppercase());
            line_start = false;
        } else {
            result.push(if line_start {
                character.to_ascii_uppercase()
            } else {
                character
            });
            line_start = character == '\n';
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{department, department_ancestors, selector_matches};

    #[test]
    fn department_is_every_segment_but_the_last() {
        assert_eq!(department("Layout/LineLength"), "Layout");
        assert_eq!(
            department("Chef/Correctness/ServiceResource"),
            "Chef/Correctness"
        );
        assert_eq!(department("Syntax"), "Syntax");
    }

    #[test]
    fn a_selector_names_a_cop_by_full_name_or_whole_department() {
        assert!(selector_matches("Layout/LineLength", "Layout/LineLength"));
        assert!(selector_matches("Layout", "Layout/LineLength"));
        assert!(!selector_matches("Lay", "Layout/LineLength"));
        assert!(!selector_matches("Layout/Line", "Layout/LineLength"));
    }

    /// An outer namespace is not a department of its own, so it selects nothing below it.
    #[test]
    fn an_outer_namespace_does_not_select_a_nested_cop() {
        let cop = "Chef/Correctness/ServiceResource";
        assert!(selector_matches("Chef/Correctness", cop));
        assert!(!selector_matches("Chef", cop));
        assert!(selector_matches(cop, cop));
    }

    /// Plugin ownership is the one lookup that widens past the cop's own department, so the
    /// ancestors run from that department outwards and stop at the outermost namespace.
    #[test]
    fn ancestors_run_from_the_department_outwards() {
        let ancestors: Vec<&str> =
            department_ancestors("Chef/Correctness/ServiceResource").collect();
        assert_eq!(ancestors, ["Chef/Correctness", "Chef"]);

        // A cop with a single department yields just that department.
        let ancestors: Vec<&str> = department_ancestors("Layout/LineLength").collect();
        assert_eq!(ancestors, ["Layout"]);

        // A cop with no department at all is its own only entry.
        let ancestors: Vec<&str> = department_ancestors("Syntax").collect();
        assert_eq!(ancestors, ["Syntax"]);
    }
}
