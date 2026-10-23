# frozen_string_literal: true

# parser gem の racc 表と Ragel の字句表を Rust の生成ファイルへ書き出す。
#
#   ruby scripts/legacy_parser/generate.rb [出力先 (既定: src/legacy_parser/tables)]
#
# RuboCop 1.89.0 は TargetRubyVersion 2.7〜3.2 の構文を parser gem の `Parser::Ruby27` /
# `Ruby30` / `Ruby31` / `Ruby32` で読む。構文エラーの判定と誤り回復をそのまま再現するため、
# 文法の表は推測で組み立てず、インストール済みの gem が持つ表を読み出す。
#
# - racc の表 (`Racc_arg`)、終端記号の名前、規則ごとの長さと左辺をそのまま書き出す。
# - 規則の動作 (`_reduce_N`) は本文を正規化して SHA-256 の先頭 8 桁で名前を付け、`Action` の
#   列挙子にする。Rust 側の移植 (`src/legacy_parser/actions.rs`) はこの列挙子を網羅的に照合する
#   ので、gem の更新で動作の本文が変われば、移植し直すまでコンパイルが通らない。
# - Ragel の表 (`Parser::Lexer` と `Parser::LexerStrings`) と、動作の番号を持つ生成元ファイルの
#   SHA-256 を書き出す。字句解析器の移植 (`src/legacy_parser/lexer`) は同じ値を定数で持ち、
#   テストで照合する。
#
# 出力は rustfmt を通すので、`cargo fmt --check` とそのまま一致する。

require "digest"
require "prism"
require "racc/parser"
require "parser"

PARSER_VERSION = "3.3.12.0"
RACC_VERSION = "1.8.1"
VERSIONS = %w[27 30 31 32 33 34].freeze

unless Parser::VERSION == PARSER_VERSION
  abort "parser #{PARSER_VERSION} が必要 (読み込まれたのは #{Parser::VERSION})"
end
racc_version = Gem.loaded_specs["racc"]&.version&.to_s
unless racc_version == RACC_VERSION
  abort "racc #{RACC_VERSION} が必要 (読み込まれたのは #{racc_version.inspect})"
end

out_dir = ARGV.fetch(0, File.join(__dir__, "..", "..", "src", "legacy_parser", "tables"))
gem_lib = File.dirname($LOADED_FEATURES.find { |f| f.end_with?("/lib/parser.rb") })

def sha256(path)
  Digest::SHA256.file(path).hexdigest
end

HEADER = <<~RUST.freeze
  //! **`scripts/legacy_parser/generate.rb` の生成物。手で変更しない。**
  //!
  //! parser #{PARSER_VERSION} (racc #{RACC_VERSION}) の表と動作を読み出したもの。
  //! 表を再構成せず gem から取得する理由は生成スクリプトを参照。
RUST

# 正規化した動作本文。racc が末尾に足す `result` の行と、行全体のコメントと空行を除く。
def normalize_action(body)
  lines = body.lines.map(&:strip)
  lines.pop while lines.last == "" || lines.last == "result"
  lines.reject { |line| line.empty? || line.start_with?("#") }.join("\n")
end

def action_bodies(path)
  result = Prism.parse_file(path)
  abort "#{path} を読めない: #{result.errors.map(&:message).join(", ")}" unless result.success?
  bodies = {}
  stack = [result.value]
  until stack.empty?
    node = stack.pop
    next unless node

    if node.is_a?(Prism::DefNode) && node.name.to_s.match?(/\A_reduce_\d+\z/)
      bodies[node.name.to_s] = normalize_action(node.body ? node.body.slice : "")
      next
    end
    stack.concat(node.compact_child_nodes)
  end
  bodies
end

def action_name(normalized)
  "A#{Digest::SHA256.hexdigest(normalized)[0, 8]}"
end

def rust_array(type, values)
  "[#{values.join(", ")}]"
end

def nil_as(values, sentinel)
  values.map { |value| value.nil? ? sentinel : value }
end

terminal_names = []
grammars = {}
actions = {}

VERSIONS.each do |version|
  require "parser/ruby#{version}"
  klass = Parser.const_get("Ruby#{version}")
  path = File.join(gem_lib, "parser", "ruby#{version}.rb")
  arg = klass::Racc_arg
  action_table, action_check, action_default, action_pointer,
    goto_table, goto_check, goto_default, goto_pointer,
    nt_base, reduce_table, token_table, shift_n, reduce_n, use_result_var = arg
  abort "ruby#{version}: use_result_var が偽" unless use_result_var

  names = klass::Racc_token_to_s_table
  token_table.each do |symbol, id|
    next if symbol == false || symbol == :error

    terminal_names << symbol.to_s unless terminal_names.include?(symbol.to_s)
    abort "ruby#{version}: 終端記号 #{symbol} の番号が名前表と合わない" unless names[id] == symbol.to_s
  end

  bodies = action_bodies(path)
  rules = reduce_table.each_slice(3).map do |length, lhs, method|
    method = method.to_s
    action = if method == "_reduce_none" || method == "racc_error"
               "None"
             else
               body = bodies.fetch(method) { abort "ruby#{version}: #{method} の本文が無い" }
               name = action_name(body)
               (actions[name] ||= { body: body, users: [] })[:users] << "ruby#{version} #{method.delete_prefix("_reduce_")} (#{names[lhs]})"
               name
             end
    [length, lhs, action]
  end
  abort "ruby#{version}: 規則の数が合わない" unless rules.size == reduce_n

  grammars[version] = {
    path: path,
    action_table: action_table, action_check: action_check, action_default: action_default,
    action_pointer: action_pointer, goto_table: goto_table, goto_check: goto_check,
    goto_default: goto_default, goto_pointer: goto_pointer, nt_base: nt_base,
    token_table: token_table, shift_n: shift_n, reduce_n: reduce_n, rules: rules,
    names: names.first(nt_base)
  }
end

def write_rust(path, text)
  File.write(path, text)
  system("rustfmt", "--edition", "2024", path, exception: true)
end

# 字句解析器が出すトークン。全版の終端記号の和集合で、版ごとの表がこれを自分の番号へ引く。
tokens = HEADER.dup
tokens << "\n/// A terminal the lexer emits, named as parser's token symbols name it.\n"
tokens << "#[allow(non_camel_case_types, clippy::upper_case_acronyms)]\n"
tokens << "#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n"
tokens << "pub(crate) enum Tok {\n"
tokens << "    /// The end of the input (`false` in parser's token stream).\n    EOF,\n"
tokens << "    /// A token whose type is `nil` (an unknown percent literal), which racc reads as `error`.\n    ERROR,\n"
terminal_names.each { |name| tokens << "    #{name},\n" }
tokens << "}\n\n"
tokens << "impl Tok {\n"
tokens << "    pub(crate) const COUNT: usize = #{terminal_names.size + 2};\n\n"
tokens << "    /// 構文診断に使うトークン名。\n"
tokens << "    pub(crate) fn name(self) -> &'static str {\n        NAMES[self as usize]\n    }\n}\n\n"
tokens << "static NAMES: [&str; Tok::COUNT] = #{rust_array("&str", ['"$end"', '"error"'] + terminal_names.map { |n| "\"#{n}\"" })};\n"
write_rust(File.join(out_dir, "tokens.rs"), tokens)

# 動作の列挙子。本文を doc に残し、どの版のどの規則が使うかも書く。
action_rs = HEADER.dup
action_rs << "\n/// 文法動作の本文のハッシュで識別する。各項目に原文を引用する。\n"
action_rs << "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n"
action_rs << "pub(crate) enum Action {\n"
action_rs << "    /// 動作の無い規則では最初の記号の値を返す。\n    None,\n"
actions.keys.sort.each do |name|
  entry = actions[name]
  action_rs << "    /// 使用箇所: #{entry[:users].first(4).join(", ")}#{entry[:users].size > 4 ? " and #{entry[:users].size - 4} more" : ""}.\n"
  action_rs << "    ///\n    /// ```ruby\n"
  entry[:body].each_line { |line| action_rs << "    /// #{line.chomp}\n".gsub(/ +\n/, "\n") }
  action_rs << "    /// ```\n"
  action_rs << "    #{name},\n"
end
action_rs << "}\n"
write_rust(File.join(out_dir, "actions.rs"), action_rs)

grammars.each do |version, g|
  token_ids = [0, 1] + terminal_names.map { |name| g[:token_table][name.to_sym] || 1 }
  rs = HEADER.dup
  rs << "//!\n//! `lib/parser/ruby#{version}.rb` sha256 #{sha256(g[:path])}\n\n"
  rs << "use super::{Grammar, Rule};\nuse super::actions::Action;\n\n"
  rs << "/// `Parser::Ruby#{version}`.\n"
  rs << "pub(crate) static GRAMMAR: Grammar = Grammar {\n"
  rs << "    action_table: &ACTION_TABLE,\n    action_check: &ACTION_CHECK,\n"
  rs << "    action_default: &ACTION_DEFAULT,\n    action_pointer: &ACTION_POINTER,\n"
  rs << "    goto_table: &GOTO_TABLE,\n    goto_check: &GOTO_CHECK,\n"
  rs << "    goto_default: &GOTO_DEFAULT,\n    goto_pointer: &GOTO_POINTER,\n"
  rs << "    nt_base: #{g[:nt_base]},\n    shift_n: #{g[:shift_n]},\n    reduce_n: #{g[:reduce_n]},\n"
  rs << "    rules: &RULES,\n    token_ids: &TOKEN_IDS,\n    names: &NAMES,\n};\n\n"
  {
    "ACTION_TABLE" => [:action_table, "i16"], "ACTION_CHECK" => [:action_check, "i16"],
    "ACTION_DEFAULT" => [:action_default, "i16"], "ACTION_POINTER" => [:action_pointer, "i16"],
    "GOTO_TABLE" => [:goto_table, "i16"], "GOTO_CHECK" => [:goto_check, "i16"],
    "GOTO_DEFAULT" => [:goto_default, "i16"], "GOTO_POINTER" => [:goto_pointer, "i16"]
  }.each do |const, (key, type)|
    values = nil_as(g[key], "NONE")
    values.each { |v| abort "#{const} が i16 に収まらない" if v.is_a?(Integer) && !(-32_767..32_767).cover?(v) }
    rs << "static #{const}: [#{type}; #{values.size}] = #{rust_array(type, values)};\n\n"
  end
  rs << "const NONE: i16 = super::NONE;\n\n"
  rs << "static RULES: [Rule; #{g[:rules].size}] = [\n"
  g[:rules].each do |length, lhs, action|
    rs << "    Rule { len: #{length}, lhs: #{lhs}, action: Action::#{action} },\n"
  end
  rs << "];\n\n"
  rs << "/// 各 Tok の終端記号番号。この文法に無いトークンは1 (error)。\n"
  rs << "static TOKEN_IDS: [u16; #{token_ids.size}] = #{rust_array("u16", token_ids)};\n\n"
  rs << "/// Racc_token_to_s_table と同じ終端記号番号ごとの名前。\n"
  rs << "static NAMES: [&str; #{g[:names].size}] = #{rust_array("&str", g[:names].map { |n| "\"#{n.gsub("\\", "\\\\\\\\").gsub('"', '\\"')}\"" })};\n"
  write_rust(File.join(out_dir, "ruby#{version}.rs"), rs)
end

# Ragel の表。動作の番号は生成元ファイルの case 文と対になるので、そのファイルの SHA-256 も残す。
{
  "lexer" => [Parser::Lexer, "lexer-F1.rb"],
  "lexer_strings" => [Parser::LexerStrings, "lexer-strings.rb"]
}.each do |name, (klass, file)|
  path = File.join(gem_lib, "parser", file)
  rs = HEADER.dup
  rs << "//!\n//! `lib/parser/#{file}`.\n\n"
  rs << "/// 生成元の SHA-256: `lib/parser/#{file}`, whose action numbers these tables index.\n"
  rs << "pub(crate) const SOURCE_SHA256: &str = \"#{sha256(path)}\";\n\n"
  klass.singleton_methods.grep(/\Alex_(en_\w+|start|error)\z/).sort.each do |method|
    rs << "pub(crate) const #{method.to_s.delete_prefix("lex_").upcase}: usize = #{klass.send(method)};\n"
  end
  rs << "\n"
  tables = {
    "TRANS_KEYS" => [:_lex_trans_keys, "u8"],
    "KEY_SPANS" => [:_lex_key_spans, "u8"],
    "INDEX_OFFSETS" => [:_lex_index_offsets, "u16"],
    "INDICIES" => [:_lex_indicies, "u16"],
    "TRANS_TARGS" => [:_lex_trans_targs, "u16"],
    "TRANS_ACTIONS" => [:_lex_trans_actions, "u16"],
    "TO_STATE_ACTIONS" => [:_lex_to_state_actions, "u16"],
    "FROM_STATE_ACTIONS" => [:_lex_from_state_actions, "u16"],
    "EOF_TRANS" => [:_lex_eof_trans, "u16"]
  }
  tables["ACTIONS"] = [:_lex_actions, "u16"] if klass.respond_to?(:_lex_actions, true)
  tables.each do |const, (method, type)|
    values = klass.send(method)
    limit = type == "u8" ? 255 : 65_535
    values.each { |v| abort "#{name} #{const} が #{type} に収まらない" unless v.is_a?(Integer) && v.between?(0, limit) }
    rs << "pub(crate) static #{const}: [#{type}; #{values.size}] = #{rust_array(type, values)};\n\n"
  end
  write_rust(File.join(out_dir, "#{name}.rs"), rs)
end

# 本体の字句解析器の動作 (`advance` の `%% write exec;`)。lexer_translate.rb が生成物の Ruby を写す。
require_relative "lexer_translate"
exec_path = File.join(gem_lib, "parser", "lexer-F1.rb")
exec_rs = HEADER.dup
exec_rs << "//!\n//! lib/parser/lexer-F1.rb の遷移動作の翻訳。翻訳器:\n"
exec_rs << "//! `scripts/legacy_parser/lexer_translate.rb`.\n\n"
exec_rs << "use super::lexer::*;\n"
exec_rs << "use super::tokens::Tok;\n"
exec_rs << "use crate::legacy_parser::Level;\n"
exec_rs << "use crate::legacy_parser::lexer::*;\n"
exec_rs << "use crate::legacy_parser::parser::Fatal;\n"
exec_rs << "use crate::legacy_parser::value::TokenValue;\n\n"
exec_rs << LegacyLexerTranslate.translate_main(exec_path, Parser::MESSAGES)
write_rust(File.join(out_dir, "lexer_exec.rs"), exec_rs)

puts "terminals=#{terminal_names.size} actions=#{actions.size}"
grammars.each { |version, g| puts "ruby#{version}: states=#{g[:shift_n]} rules=#{g[:reduce_n]}" }
