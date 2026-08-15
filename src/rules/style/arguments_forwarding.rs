//! `def foo(*args, &block); bar(*args, &block); end` written as `def foo(...); bar(...); end`.
//!
//! What can be forwarded depends on the target version: `...` arrived in 2.7, taking arguments
//! before it in 3.0, and the anonymous `&` and `*` / `**` in 3.1 and 3.2. The cop therefore has
//! two paths -- one that folds everything into `...`, one that anonymises each of the three on its
//! own -- and picks between them by what every call in the body forwards.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

/// `minimum_target_ruby_version 2.7`: `...` arrived in 2.7.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);
const RUBY_3_0: RubyVersion = RubyVersion::new(3, 0);
const RUBY_3_2: RubyVersion = RubyVersion::new(3, 2);
const RUBY_3_4: RubyVersion = RubyVersion::new(3, 4);

const FORWARDING_MSG: &str = "Use shorthand syntax `...` for arguments forwarding.";
const ARGS_MSG: &str = "Use anonymous positional arguments forwarding (`*`).";
const KWARGS_MSG: &str = "Use anonymous keyword arguments forwarding (`**`).";
const BLOCK_MSG: &str = "Use anonymous block arguments forwarding (`&`).";

/// The three parameters that can be forwarded, each kept only when its name is one the
/// configuration calls meaningless -- a name worth reading is a reason to leave the call alone.
#[derive(Clone, Copy, Default)]
struct Forwardable<'tree> {
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

/// `classification`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Classification {
    All,
    AllAnonymous,
    RestOrKwrest,
}

/// One call of the body, with what it turned out to forward.
struct Classified<'tree> {
    send: Node<'tree>,
    classification: Classification,
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let cop = Cop {
        context,
        version: context.target_ruby_version(),
        allow_only_rest_arguments: context.setting("AllowOnlyRestArgument").unwrap_or(true),
        use_anonymous_forwarding: context.setting("UseAnonymousForwarding").unwrap_or(false),
        rest_names: names(context, "RedundantRestArgumentNames"),
        kwrest_names: names(context, "RedundantKeywordRestArgumentNames"),
        block_names: names(context, "RedundantBlockArgumentNames"),
        explicit_block_name: context
            .setting_of::<String>("Naming/BlockForwarding", "EnforcedStyle")
            .is_some_and(|style| style == "explicit"),
    };
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        cop.on_def(node, offenses);
    }
}

fn names(context: &RuleContext<'_>, key: &str) -> Vec<String> {
    context.setting::<Vec<String>>(key).unwrap_or_default()
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    version: RubyVersion,
    allow_only_rest_arguments: bool,
    use_anonymous_forwarding: bool,
    rest_names: Vec<String>,
    kwrest_names: Vec<String>,
    block_names: Vec<String>,
    explicit_block_name: bool,
}

impl<'tree> Cop<'_, 'tree> {
    fn on_def(&self, node: Node<'tree>, offenses: &mut Vec<Offense>) {
        let Some(body) = node.field("body") else {
            return;
        };
        let parameters = self.parameters(node);
        let forwardable = self.redundant_forwardable_named_args(&parameters);
        let referenced = self.referenced_names(body, &forwardable);
        let classifications: Vec<Classified<'tree>> = self
            .send_nodes(body)
            .into_iter()
            .filter_map(|send| self.classify(node, send, &parameters, forwardable, &referenced))
            .collect();
        if classifications.is_empty() {
            return;
        }
        // `only_forwards_all?`: every call takes the whole lot, so `...` says it for all of them.
        if classifications.iter().all(|classified| {
            matches!(
                classified.classification,
                Classification::All | Classification::AllAnonymous
            )
        }) {
            self.add_forward_all_offenses(node, &classifications, forwardable, offenses);
        } else if self.version >= RUBY_3_2 {
            self.add_post_ruby_32_offenses(node, &classifications, forwardable, offenses);
        }
    }

    /// `node.arguments` for a definition: the parameters it was written with.
    fn parameters(&self, node: Node<'tree>) -> Vec<Node<'tree>> {
        node.field("parameters")
            .map(|list| {
                named_children(list)
                    .into_iter()
                    .filter(|child| child.kind_str() != "comment")
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `redundant_forwardable_named_args`: each of the three, kept only when its name is one the
    /// configuration lists -- or when it was already written anonymously.
    fn redundant_forwardable_named_args(&self, parameters: &[Node<'tree>]) -> Forwardable<'tree> {
        let find = |kind: &str| parameters.iter().copied().find(|p| p.kind_str() == kind);
        Forwardable {
            rest: self.redundant(find("splat_parameter"), &self.rest_names, "*"),
            kwrest: self.redundant(find("hash_splat_parameter"), &self.kwrest_names, "**"),
            block: self.redundant(find("block_parameter"), &self.block_names, "&"),
        }
    }

    /// `redundant_named_arg`.
    fn redundant(
        &self,
        parameter: Option<Node<'tree>>,
        allowed: &[String],
        keyword: &str,
    ) -> Option<Node<'tree>> {
        let parameter = parameter?;
        let source = self.context.source.node_text(parameter);
        let redundant = source == keyword
            || allowed
                .iter()
                .any(|name| source == format!("{keyword}{name}"));
        redundant.then_some(parameter)
    }

    /// `node.each_descendant(:call, :super, :yield)`.
    ///
    /// A bare `super` is a `zsuper` upstream and no part of that set; here it is a node of its own
    /// and drops out by kind. `super(…)` and `x[…]` are both sends there, and both are a kind of
    /// their own here.
    fn send_nodes(&self, body: Node<'tree>) -> Vec<Node<'tree>> {
        let mut found = Vec::new();
        let mut stack = vec![body];
        while let Some(node) = stack.pop() {
            if matches!(node.kind_str(), "call" | "yield" | "element_reference") {
                found.push(node);
            }
            crate::rules::push_named_children(node, &mut stack);
        }
        found.sort_by_key(Node::start_byte);
        found
    }

    /// `non_splat_or_block_pass_lvar_references`: the names the body reads or writes somewhere
    /// other than as the `*x`, `**x` or `&x` of a call, which is what forwarding would take away.
    fn referenced_names(&self, body: Node<'tree>, forwardable: &Forwardable<'tree>) -> Vec<String> {
        let watched: Vec<String> = [forwardable.rest, forwardable.kwrest, forwardable.block]
            .into_iter()
            .flatten()
            .filter_map(|parameter| self.parameter_name(parameter))
            .collect();
        let mut referenced = Vec::new();
        let mut stack = vec![body];
        while let Some(node) = stack.pop() {
            if node.kind_str() == "identifier" {
                let text = self.context.source.node_text(node);
                if watched.iter().any(|name| name == text)
                    && !referenced.iter().any(|name| name == text)
                    && self.is_bare_reference(node)
                {
                    referenced.push(text.to_owned());
                }
            }
            crate::rules::push_named_children(node, &mut stack);
        }
        referenced
    }

    /// Whether the name stands where upstream would have built an `lvar` or an `lvasgn` rather
    /// than the forwarding argument the cop is about to take away.
    fn is_bare_reference(&self, node: Node<'tree>) -> bool {
        let Some(parent) = node.parent_of(self.context) else {
            return true;
        };
        match parent.kind_str() {
            // `*x`, `**x` and `&x` in an argument list are the forwarding itself.
            "splat_argument" | "hash_splat_argument" | "block_argument" => false,
            // The name of a call is a symbol upstream, not a variable.
            "call" => parent
                .field("method")
                .is_none_or(|method| method.id() != node.id()),
            _ => true,
        }
    }

    fn parameter_name(&self, parameter: Node<'tree>) -> Option<String> {
        parameter
            .field("name")
            .map(|name| self.context.source.node_text(name).to_owned())
    }

    /// `SendNodeClassifier`.
    fn classify(
        &self,
        def: Node<'tree>,
        send: Node<'tree>,
        parameters: &[Node<'tree>],
        forwardable: Forwardable<'tree>,
        referenced: &[String],
    ) -> Option<Classified<'tree>> {
        let list = send_arguments(send);
        let name = |parameter: Option<Node<'tree>>| parameter.and_then(|p| self.parameter_name(p));
        let (rest_name, kwrest_name, block_name) = (
            name(forwardable.rest),
            name(forwardable.kwrest),
            name(forwardable.block),
        );
        let referenced_rest = self.is_referenced(&rest_name, referenced);
        let referenced_kwrest = self.is_referenced(&kwrest_name, referenced);
        let referenced_block = self.is_referenced(&block_name, referenced);

        let rest = (!referenced_rest)
            .then(|| self.forwarded_rest(&list, rest_name.as_deref()))
            .flatten();
        let kwrest = (!referenced_kwrest)
            .then(|| self.forwarded_kwrest(&list, kwrest_name.as_deref()))
            .flatten();
        let block = (!referenced_block)
            .then(|| self.forwarded_block(&list, block_name.as_deref()))
            .flatten();
        if rest.is_none() && kwrest.is_none() && block.is_none() {
            return None;
        }

        let any_referenced = referenced_rest || referenced_kwrest || referenced_block;
        let classification = if self.ruby_32_only_anonymous_forwarding(def, send, parameters, &list)
        {
            Classification::AllAnonymous
        } else if self.can_forward_all(
            def,
            parameters,
            &list,
            forwardable,
            (rest, kwrest, block),
            (rest_name.is_some(), kwrest_name.is_some()),
            any_referenced,
        ) {
            Classification::All
        } else {
            Classification::RestOrKwrest
        };
        Some(Classified {
            send,
            classification,
            rest,
            kwrest,
            block,
        })
    }

    fn is_referenced(&self, name: &Option<String>, referenced: &[String]) -> bool {
        name.as_ref()
            .is_some_and(|name| referenced.iter().any(|other| other == name))
    }

    /// `forwarded_rest_arg?`: `(splat (lvar %1))`.
    fn forwarded_rest(&self, list: &[Arg<'tree>], name: Option<&str>) -> Option<Node<'tree>> {
        let name = name?;
        list.iter()
            .map(Arg::first)
            .filter(|argument| argument.kind_str() == "splat_argument")
            .find(|argument| self.only_child_named(*argument, name))
    }

    /// `extract_forwarded_kwrest_arg`: `(hash <$(kwsplat (lvar %1)) ...>)`, whose match is the
    /// `**name` itself rather than the hash holding it.
    fn forwarded_kwrest(
        &self,
        list: &[Arg<'tree>],
        name: Option<&str>,
    ) -> Option<Node<'tree>> {
        let name = name?;
        list.iter()
            .flat_map(hash_members)
            .filter(|member| member.kind_str() == "hash_splat_argument")
            .find(|member| self.only_child_named(*member, name))
    }

    /// `forwarded_block_arg?`: `(block_pass {(lvar %1) nil?})`.
    ///
    /// The `nil?` half asks nothing of the name, so a bare `&` is forwarding whatever the block
    /// parameter was called -- including a `&` that was written anonymously to begin with.
    fn forwarded_block(&self, list: &[Arg<'tree>], name: Option<&str>) -> Option<Node<'tree>> {
        list.iter()
            .map(Arg::first)
            .filter(|argument| argument.kind_str() == "block_argument")
            .find(|argument| {
                named_children(*argument).is_empty()
                    || name.is_some_and(|name| self.only_child_named(*argument, name))
            })
    }

    fn only_child_named(&self, node: Node<'tree>, name: &str) -> bool {
        matches!(named_children(node).as_slice(),
            [only] if only.kind_str() == "identifier"
                && self.context.source.node_text(*only) == name)
    }

    /// `ruby_32_only_anonymous_forwarding?`: everything was written anonymously on both sides
    /// already, which `...` can still shorten.
    fn ruby_32_only_anonymous_forwarding(
        &self,
        def: Node<'tree>,
        send: Node<'tree>,
        parameters: &[Node<'tree>],
        list: &[Arg<'tree>],
    ) -> bool {
        // An anonymous block argument and a named one are never passed together.
        if self.inside_block(send) {
            return false;
        }
        let _ = def;
        // `(args ... (restarg) (kwrestarg) (blockarg nil?))`.
        let anonymous_def = matches!(parameters,
            [.., rest, kwrest, block]
                if rest.kind_str() == "splat_parameter" && rest.field("name").is_none()
                && kwrest.kind_str() == "hash_splat_parameter" && kwrest.field("name").is_none()
                && block.kind_str() == "block_parameter" && block.field("name").is_none());
        // `(send _ _ ... (forwarded_restarg) (hash (forwarded_kwrestarg)) (block_pass nil?))`.
        let anonymous_send = send.kind_str() == "call"
            && matches!(list,
                [.., rest, kwrest, block]
                    if rest.first().kind_str() == "splat_argument"
                        && named_children(rest.first()).is_empty()
                    && matches!(hash_members(kwrest).as_slice(),
                        [only] if only.kind_str() == "hash_splat_argument"
                            && named_children(*only).is_empty())
                    && block.first().kind_str() == "block_argument"
                        && named_children(block.first()).is_empty());
        anonymous_def && anonymous_send
    }

    /// `can_forward_all?`.
    #[allow(clippy::too_many_arguments)]
    fn can_forward_all(
        &self,
        def: Node<'tree>,
        parameters: &[Node<'tree>],
        list: &[Arg<'tree>],
        forwardable: Forwardable<'tree>,
        forwarded: (
            Option<Node<'tree>>,
            Option<Node<'tree>>,
            Option<Node<'tree>>,
        ),
        named: (bool, bool),
        any_referenced: bool,
    ) -> bool {
        let (rest, kwrest, block) = forwarded;
        let _ = def;
        if any_referenced {
            return false;
        }
        // `def foo(a = 41, ...)` is a syntax error in 3.0.
        if self.version <= RUBY_3_0
            && parameters
                .iter()
                .any(|parameter| parameter.kind_str() == "optional_parameter")
        {
            return false;
        }
        if self.version >= RUBY_3_2 && !(rest.is_some() && kwrest.is_some()) {
            return false;
        }
        // `offensive_block_forwarding?`.
        let offensive_block = match forwardable.block {
            Some(_) => block.is_some(),
            None => !self.allow_only_rest_arguments,
        };
        if !offensive_block {
            return false;
        }
        // `additional_kwargs_or_forwarded_kwargs?`.
        let additional_kwargs = parameters.iter().any(|parameter| {
            matches!(
                parameter.kind_str(),
                "keyword_parameter" | "hash_key_symbol"
            )
        });
        let forward_additional_kwargs = kwrest.is_some_and(|kwrest| {
            list.iter()
                .any(|argument| hash_members(argument).len() > 1 && holds(argument, kwrest))
        });
        if additional_kwargs || forward_additional_kwargs {
            return false;
        }
        self.no_additional_args(parameters, list, forwardable, forwarded, named)
            || (self.version >= RUBY_3_0 && no_post_splat_args(list, rest))
    }

    /// `no_additional_args?`.
    fn no_additional_args(
        &self,
        parameters: &[Node<'tree>],
        list: &[Arg<'tree>],
        forwardable: Forwardable<'tree>,
        forwarded: (
            Option<Node<'tree>>,
            Option<Node<'tree>>,
            Option<Node<'tree>>,
        ),
        named: (bool, bool),
    ) -> bool {
        let count = [forwardable.rest, forwardable.kwrest, forwardable.block]
            .into_iter()
            .flatten()
            .count();
        // `missing_rest_arg_or_kwrest_arg?`.
        if (named.0 && forwarded.0.is_none()) || (named.1 && forwarded.1.is_none()) {
            return false;
        }
        parameters.len() == count && list.len() == count
    }

    /// `node.each_ancestor(:any_block).any?`.
    ///
    /// Upstream wraps the whole call in a `block` node, so everything the call was written with --
    /// its receiver and its arguments included -- sits inside that block. Here the block hangs off
    /// the call instead, so a call carrying one has to be read as the block it stands for.
    fn inside_block(&self, node: Node<'tree>) -> bool {
        let mut current = Some(node);
        while let Some(ancestor) = current {
            if matches!(ancestor.kind_str(), "block" | "do_block" | "lambda")
                || (ancestor.kind_str() == "call" && ancestor.field("block").is_some())
            {
                return true;
            }
            current = ancestor.parent_of(self.context);
        }
        false
    }

    /// `allow_anonymous_forwarding_in_block?`: Ruby 3.3 made an anonymous argument inside a block
    /// a syntax error, so nothing is proposed there until 3.4.
    fn allow_anonymous_forwarding_in_block(&self, node: Option<Node<'tree>>) -> bool {
        let Some(node) = node else {
            return false;
        };
        self.version >= RUBY_3_4 || !self.inside_block(node)
    }

    /// `add_forward_all_offenses`.
    fn add_forward_all_offenses(
        &self,
        def: Node<'tree>,
        classifications: &[Classified<'tree>],
        forwardable: Forwardable<'tree>,
        offenses: &mut Vec<Offense>,
    ) {
        let mut registered_block_arg_offense = false;
        for classified in classifications {
            if classified.rest.is_none()
                && classified.kwrest.is_none()
                && classified.classification != Classification::AllAnonymous
            {
                if self.allow_anonymous_forwarding_in_block(classified.block) {
                    let parens = classified.rest.is_none();
                    self.register_block(
                        parens,
                        def.field("parameters"),
                        forwardable.block,
                        offenses,
                    );
                    self.register_block(parens, Some(classified.send), classified.block, offenses);
                }
                registered_block_arg_offense = true;
                break;
            }
            let first = classified
                .rest
                .or(classified.kwrest)
                .or_else(|| forward_all_first_argument(classified.send));
            self.register_forward_all(classified.send, Some(classified.send), first, offenses);
        }
        if registered_block_arg_offense {
            return;
        }
        self.register_forward_all(
            def,
            def.field("parameters"),
            forwardable.rest.or(forwardable.kwrest),
            offenses,
        );
    }

    /// `add_post_ruby_32_offenses`.
    fn add_post_ruby_32_offenses(
        &self,
        def: Node<'tree>,
        classifications: &[Classified<'tree>],
        forwardable: Forwardable<'tree>,
        offenses: &mut Vec<Offense>,
    ) {
        if !self.use_anonymous_forwarding {
            return;
        }
        // `all_forwarding_offenses_correctable?`.
        if self.version < RUBY_3_4
            && classifications
                .iter()
                .any(|classified| self.inside_block(classified.send))
        {
            return;
        }
        let parameters = def.field("parameters");
        for classified in classifications {
            if self.allow_anonymous_forwarding_in_block(classified.rest) {
                self.register_anonymous(
                    true,
                    parameters,
                    forwardable.rest,
                    ARGS_MSG,
                    "*",
                    offenses,
                );
                self.register_anonymous(
                    true,
                    Some(classified.send),
                    classified.rest,
                    ARGS_MSG,
                    "*",
                    offenses,
                );
            }
            let parens = classified.rest.is_none();
            if self.allow_anonymous_forwarding_in_block(classified.kwrest) {
                self.register_anonymous(
                    parens,
                    parameters,
                    forwardable.kwrest,
                    KWARGS_MSG,
                    "**",
                    offenses,
                );
                self.register_anonymous(
                    parens,
                    Some(classified.send),
                    classified.kwrest,
                    KWARGS_MSG,
                    "**",
                    offenses,
                );
            }
            if self.allow_anonymous_forwarding_in_block(classified.block) {
                self.register_block(parens, parameters, forwardable.block, offenses);
                self.register_block(parens, Some(classified.send), classified.block, offenses);
            }
        }
    }

    /// `register_forward_args_offense` and `register_forward_kwargs_offense`, which differ only in
    /// what they write and whether they add the parentheses.
    fn register_anonymous(
        &self,
        parens: bool,
        holder: Option<Node<'tree>>,
        target: Option<Node<'tree>>,
        message: &str,
        replacement: &str,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(target) = target else {
            return;
        };
        let mut edits = vec![Edit {
            start: target.start_byte(),
            end: target.end_byte(),
            replacement: replacement.to_owned(),
            safe: true,
        }];
        if parens && let Some(holder) = holder {
            edits.extend(self.add_parentheses(holder));
        }
        offenses.push(
            self.context
                .offense(message, target.byte_range())
                .corrected_by_all(edits),
        );
    }

    /// `register_forward_block_arg_offense`.
    fn register_block(
        &self,
        parens: bool,
        holder: Option<Node<'tree>>,
        target: Option<Node<'tree>>,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(block) = target else {
            return;
        };
        if self.version <= RUBY_3_0
            || self.context.source.node_text(block) == "&"
            || self.explicit_block_name
        {
            return;
        }
        self.register_anonymous(parens, holder, Some(block), BLOCK_MSG, "&", offenses);
    }

    /// `register_forward_all_offense`.
    fn register_forward_all(
        &self,
        holder: Node<'tree>,
        parens_target: Option<Node<'tree>>,
        first: Option<Node<'tree>>,
        offenses: &mut Vec<Offense>,
    ) {
        let (Some(first), Some(last)) = (first, self.last_argument(holder)) else {
            return;
        };
        let range = first.start_byte()..last.end_byte();
        let mut edits = vec![Edit {
            start: range.start,
            end: range.end,
            replacement: "...".to_owned(),
            safe: true,
        }];
        if let Some(target) = parens_target {
            edits.extend(self.add_parentheses(target));
        }
        offenses.push(
            self.context
                .offense(FORWARDING_MSG, range)
                .corrected_by_all(edits),
        );
    }

    /// `node.last_argument`, of a definition or of a call.
    fn last_argument(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        match node.kind_str() {
            "method" | "singleton_method" => self.parameters(node).last().copied(),
            _ => send_arguments(node)
                .last()
                .map(|argument| argument.last()),
        }
    }

    /// `add_parentheses_if_missing`, as the edits it would have asked for.
    fn add_parentheses(&self, node: Node<'tree>) -> Vec<Edit> {
        if self.parenthesized(node) {
            return Vec::new();
        }
        // `x[1]` is a send whose "parentheses" are its brackets.
        if node.kind_str() == "element_reference" {
            return Vec::new();
        }
        // An `args` node: the space before the parameters becomes the `(`.
        if node.kind_str() == "method_parameters" {
            let range = node.byte_range();
            let leading =
                super::ranges::extended_left(self.context.source.text(), range.start, true);
            return vec![
                Edit {
                    start: leading,
                    end: range.start,
                    replacement: "(".to_owned(),
                    safe: true,
                },
                Edit {
                    start: range.end,
                    end: range.end,
                    replacement: ")".to_owned(),
                    safe: true,
                },
            ];
        }
        let Some(selector) = self.selector(node) else {
            return Vec::new();
        };
        let list = send_arguments(node);
        let Some(last) = self.last_argument(node) else {
            return Vec::new();
        };
        let _ = list;
        let text = self.context.source.text();
        let begin = text[selector.end_byte()..]
            .char_indices()
            .nth(1)
            .map_or(text.len(), |(offset, _)| selector.end_byte() + offset);
        vec![
            Edit {
                start: selector.end_byte(),
                end: begin,
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: last.end_byte(),
                end: last.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            },
        ]
    }

    /// `parentheses?`: the node ends with a `)`.
    fn parenthesized(&self, node: Node<'tree>) -> bool {
        let range = match node.kind_str() {
            "method_parameters" => node.byte_range(),
            _ => match node.field("arguments") {
                Some(list) => list.byte_range(),
                None => match named_children(node)
                    .into_iter()
                    .find(|child| child.kind_str() == "argument_list")
                {
                    Some(list) => list.byte_range(),
                    None => return false,
                },
            },
        };
        self.context.source.slice(range).ends_with(')')
    }

    /// `loc.selector`, or `loc.keyword` for a `yield`.
    fn selector(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        match node.kind_str() {
            "call" => node.field("method"),
            "yield" => node.child(0),
            _ => None,
        }
    }
}

/// `forward_all_first_argument`: the last `*` written anonymously.
fn forward_all_first_argument<'tree>(send: Node<'tree>) -> Option<Node<'tree>> {
    send_arguments(send)
        .into_iter()
        .map(|argument| argument.first())
        .rfind(|argument| {
            argument.kind_str() == "splat_argument" && named_children(*argument).is_empty()
        })
}

/// The members of an argument that upstream would have built a `hash` for, and nothing for any
/// other argument.
fn hash_members<'tree>(argument: &Arg<'tree>) -> Vec<Node<'tree>> {
    let parts = argument.parts();
    if parts.len() > 1 || matches!(parts[0].kind_str(), "pair" | "hash_splat_argument") {
        return parts.to_vec();
    }
    match parts[0].kind_str() {
        "hash" => named_children(parts[0]),
        _ => Vec::new(),
    }
}

/// Whether the argument is the one holding that node.
fn holds(argument: &Arg<'_>, node: Node<'_>) -> bool {
    hash_members(argument)
        .into_iter()
        .any(|member| member.id() == node.id())
}

/// `no_post_splat_args?`: nothing but a hash or a block pass was written after the `*`.
fn no_post_splat_args(list: &[Arg<'_>], rest: Option<Node<'_>>) -> bool {
    let Some(rest) = rest else {
        return true;
    };
    let Some(index) = list
        .iter()
        .position(|argument| argument.first().id() == rest.id())
    else {
        return true;
    };
    match list.get(index + 1) {
        None => true,
        Some(next) => {
            let first = next.first();
            matches!(first.kind_str(), "block_argument" | "hash") || !hash_members(next).is_empty()
        }
    }
}

/// One argument as upstream's parser groups them: a trailing run of `key: value` pairs and
/// `**splat`s is the one `hash` node it builds, not several arguments.
struct Arg<'tree> {
    parts: Vec<Node<'tree>>,
}

impl<'tree> Arg<'tree> {
    fn first(&self) -> Node<'tree> {
        self.parts[0]
    }

    fn last(&self) -> Node<'tree> {
        self.parts[self.parts.len() - 1]
    }

    fn parts(&self) -> &[Node<'tree>] {
        &self.parts
    }
}

/// `node.arguments`, for each of the three kinds this cop walks. `x[…]` and `yield …` are sends
/// upstream, so their arguments are read the same way a call's are.
fn send_arguments<'tree>(node: Node<'tree>) -> Vec<Arg<'tree>> {
    let written: Vec<Node<'tree>> = match node.kind_str() {
        "call" | "yield" => match argument_list(node) {
            Some(list) => named_children(list),
            None => Vec::new(),
        },
        "element_reference" => named_children(node)
            .into_iter()
            .filter(|child| {
                node.field("object")
                    .is_none_or(|object| object.id() != child.id())
            })
            .collect(),
        _ => Vec::new(),
    };
    fold(written)
}

fn argument_list<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("arguments").or_else(|| {
        named_children(node)
            .into_iter()
            .find(|child| child.kind_str() == "argument_list")
    })
}

fn fold<'tree>(written: Vec<Node<'tree>>) -> Vec<Arg<'tree>> {
    let mut arguments: Vec<Arg<'tree>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for node in written {
        if node.kind_str() == "comment" {
            continue;
        }
        if matches!(node.kind_str(), "pair" | "hash_splat_argument") {
            hash.push(node);
            continue;
        }
        if !hash.is_empty() {
            arguments.push(Arg {
                parts: std::mem::take(&mut hash),
            });
        }
        arguments.push(Arg { parts: vec![node] });
    }
    if !hash.is_empty() {
        arguments.push(Arg { parts: hash });
    }
    arguments
}
