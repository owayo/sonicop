# frozen_string_literal: true

# Dumps, for every distinct regexp literal of the given trees, the expression list
# `Regexp::Parser` builds for it. The result is the fixture
# `tests/fixtures/regexp_trees.jsonl`, which `rules::lint::regexp_tree` is checked against.
#
#   ruby scripts/dump_regexp_trees.rb ~/tmp/source/rubocop_rubocop … > tests/fixtures/regexp_trees.jsonl
#
# One line per pattern: `1` or `0` for the `x` flag, a tab, the pattern, a tab, then
# `kind|token|ts|te|quantifier` for each node of `each_expression(true)`, joined by U+001F. The pattern has its line breaks, tabs and backslashes
# escaped so that one pattern stays on one line.

require 'rubocop'
require 'set'

def escape(text)
  text.gsub('\\', '\\\\\\\\').gsub("\n", '\\n').gsub("\t", '\\t').gsub("\r", '\\r')
end

seen = Set.new
ARGV.each do |root|
  Dir.glob(File.join(root, '**', '*.rb')).sort.each do |path|
    source = begin
      File.read(path)
    rescue StandardError
      next
    end
    processed = begin
      RuboCop::ProcessedSource.new(source, 3.3, path)
    rescue StandardError
      next
    end
    next unless processed.valid_syntax?

    ast = processed.ast
    next unless ast

    nodes = ast.regexp_type? ? [ast] : ast.each_descendant(:regexp).to_a
    nodes.each do |node|
      tree = node.parsed_tree
      next unless tree

      # The pattern as the gem saw it: interpolations blanked to spaces of the same width, which
      # is what `RegexpNode#assign_properties` feeds it.
      pattern = node.children[0...-1].map { |child|
        child.begin_type? ? ' ' * child.source.length : child.source
      }.join
      next unless seen.add?(pattern)

      entries = []
      tree.each_expression(true) do |expression, _index|
        quantifier = expression.quantifier
        # The token of a Unicode property is normalised through the gem's alias table, which no
        # cop reads; leaving it out keeps `regexp_tree` from having to port that table.
        token = %i[property nonproperty].include?(expression.type) ? '' : expression.token
        entries << [expression.type, token, expression.ts, expression.te,
                    quantifier ? quantifier.text : ''].join('|')
      end
      extended = (node.regopt.source.include?('x') ? '1' : '0')
      puts "#{extended}\t#{escape(pattern)}\t#{entries.join("\u001f")}"
    end
  end
end
