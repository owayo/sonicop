use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_string, string_text, symbol_name};

use super::support::declarations;

const HTTP_SOURCE: &str = "http://rubygems.org";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_http: bool = context.setting("AllowHttpProtocol").unwrap_or(true);
    for node in declarations(context, "source") {
        // `(send nil? :source ${(sym :gemcutter) (sym :rubygems) (sym :rubyforge)
        // (:str "http://rubygems.org")})`: exactly one argument, and one of those four.
        let arguments = arguments(node);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        let source = argument.first();
        let message = if let Some(name) = symbol_name(source, context)
            && matches!(name, "gemcutter" | "rubygems" | "rubyforge")
        {
            format!(
                "The source `:{name}` is deprecated because HTTP requests are insecure. \
                 Please change your source to 'https://rubygems.org' if possible, \
                 or 'http://rubygems.org' if not."
            )
        } else if is_string(source, context) && string_text(source, context) == HTTP_SOURCE {
            if allow_http {
                continue;
            }
            "Use `https://rubygems.org` instead of `http://rubygems.org`.".to_owned()
        } else {
            continue;
        };
        offenses.push(
            context
                .offense(message, source.byte_range())
                .corrected_by(Edit {
                    start: source.start_byte(),
                    end: source.end_byte(),
                    replacement: "'https://rubygems.org'".to_owned(),
                    safe: true,
                }),
        );
    }
}
