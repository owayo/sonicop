use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(5);
    let count_keywords: bool = context.setting("CountKeywordArgs").unwrap_or(true);
    for node in context.nodes_of("method_parameters") {
        let mut cursor = node.walk();
        let count = node
            .named_children(&mut cursor)
            .filter(|parameter| count_keywords || parameter.kind() != "keyword_parameter")
            .count();
        if count <= max {
            continue;
        }
        offenses.push(context.offense(
            format!("Avoid parameter lists longer than {max} parameters. [{count}/{max}]"),
            node.byte_range(),
        ));
    }
}
