# frozen_string_literal: true

require 'json'
require 'shellwords'

# POSIX の推測値ではなく、導入済み Ruby Shellwords の返値と例外を保存する。
cases = []
{ 'space' => ' ', 'tab' => "\t", 'lf' => "\n", 'crlf' => "\r\n",
  'cr' => "\r", 'vt' => "\v", 'ff' => "\f", 'nbsp' => "\u00a0" }.each do |name, whitespace|
  { 'delimiter' => "a#{whitespace}b", 'single' => "'a#{whitespace}b'",
    'double' => "\"a#{whitespace}b\"", 'escaped' => "a\\#{whitespace}b" }.each do |context, source|
    cases << ["#{name}-#{context}", source]
  end
end
cases.concat([
  ['hash', '#value next'], ['embedded-hash', 'a#value next'], ['single-empty', "''"],
  ['double-empty', '""'], ['adjacent-empty', "a''b"], ['terminal-backslash', "a\\"],
  ['dq-slash-lf', "\"a\\\nb\""], ['uq-slash-lf', "a\\\nb"], ['nul', "a\0b"],
  ['unmatched', "'a"], ['unicode', '日本 "空 白😀"']
])
records = cases.map do |id, source|
  result = begin
    { 'words_hex' => Shellwords.split(source).map { |word| word.unpack1('H*') } }
  rescue ArgumentError => e
    { 'error_class' => e.class.name, 'error' => e.message }
  end
  { 'id' => id, 'source_hex' => source.unpack1('H*'), 'reference' => result }
end
STDOUT.binmode.write(JSON.pretty_generate({
  'ruby_version' => RUBY_VERSION,
  'shellwords_version' => Shellwords::VERSION,
  'cases' => records
}) + "\n")
