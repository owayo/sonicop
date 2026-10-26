# frozen_string_literal: true

# 既存の全 stdin 条件を本家 CLI で動かし、実 text mode の出力と tail 保護を記録する。
# 引数は既存 nul_writeback_stdin.json と、別の出力 JSON の順。
gem 'parser', '3.3.12.0'
gem 'prism', '1.8.1'
gem 'rubocop-ast', '1.50.0'
gem 'rubocop', '1.89.0'
require 'rubocop'
require 'prism'
require 'json'
require 'tmpdir'
require 'digest'
require_relative 'oracle_environment'

abort 'Windows Ruby 4.0.7 is required' unless RUBY_VERSION == '4.0.7' && RUBY_PLATFORM.match?(/mingw|mswin/)

def tail_start(source)
  parsed = Prism.parse(source)
  if parsed.data_loc
    newline = source.index("\n", parsed.data_loc.start_offset)
    return newline ? newline + 1 : parsed.data_loc.start_offset
  end
  token, = Prism.lex(source).value.last
  offset = token.location.start_offset
  offset if token.type == :EOF && token.location.length.zero? && source.getbyte(offset) == 0
end

def native(source)
  source.gsub("\n".b, "\r\n".b)
end

source_path, output_path = ARGV
raise 'input fixture and a separate output path are required' unless ARGV.length == 2
raise 'the source fixture must not be overwritten' if File.expand_path(source_path) == File.expand_path(output_path)
source_bytes = File.binread(source_path)
records = JSON.parse(source_bytes)
raise 'the complete 252-case fixture is required' unless records.is_a?(Array) && records.length == 252
raise '168 correcting cases are required' unless records.count { |entry| entry['after_hex'] } == 168
identities = records.map { |entry| entry.values_at('name', 'ruby', 'engine', 'eol', 'mode') }
raise 'duplicate source conditions' unless identities.uniq.length == records.length
measured = {}
original_streams = [$stdin, $stdout, $stderr]
Dir.mktmpdir('sonicop-stdio-oracle') do |root|
  records.each_with_index do |entry, index|
    folder = File.join(root, index.to_s)
    Dir.mkdir(folder)
    correcting = entry.fetch('mode') != 'inspect'
    before = [entry.fetch('source_hex')].pack('H*')
    logical = correcting ? [entry.fetch('after_hex')].pack('H*') : nil
    File.binwrite(File.join(folder, 'source.bin'), before)
    configuration = File.join(root, "config-#{index}.yml")
    File.write(configuration, <<~YAML)
      AllCops:
        NewCops: disable
        TargetRubyVersion: #{entry.fetch('ruby')}
        ParserEngine: #{entry.fetch('engine')}
    YAML
    status = nil
    handles = []
    begin
      # PathUtil が process の cwd を保持するので、全 CLI の cwd は同じにする。
      # 設定ファイルのパスは条件ごとに分け、版と engine の設定キャッシュを共有しない。
      Dir.chdir(root) do
        handles = [File.open(File.join(folder, 'source.bin'), 'rb'), File.open(File.join(folder, 'stdout.bin'), 'w'), File.open(File.join(folder, 'stderr.bin'), 'w')]
        $stdin = handles[0]
        # 実 STDOUT/STDERR は File ではなく IO。formatter が標準 IO を閉じない形を保つ。
        $stdout = IO.for_fd(handles[1].fileno, 'w', autoclose: false)
        $stderr = IO.for_fd(handles[2].fileno, 'w', autoclose: false)
        status = SonicopOracleEnvironment.without_injections do
          RuboCop::CLI.new.run([
            '--config', configuration, '--cache', 'false', '--only',
            'Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile',
            *(correcting ? [entry.fetch('mode')] : []), '--stdin', 'example.rb', '--stderr', '-f', 'simple'
          ])
        end
      end
    ensure
      [$stdout, $stderr].each(&:flush)
      handles.each(&:close)
      $stdin, $stdout, $stderr = original_streams
    end
    stdout = File.binread(File.join(folder, 'stdout.bin'))
    stderr = File.binread(File.join(folder, 'stderr.bin'))
    raise "exit changed at #{index}" unless status == entry.fetch('exit') && [0, 1].include?(status)
    raise "incomplete report at #{index}" unless stderr.include?('1 file inspected,')
    raise "unexpected absolute stdin path at #{index}" if stderr.include?(root.tr('\\', '/'))
    raise "unexpected source separator at #{index}" unless stderr.end_with?("====================\r\n") == correcting
    raise "upstream internal error at #{index}" if stderr.match?(/An error occurred while|\d+ errors? occurred/)
    raise "logical buffer changed at #{index}" unless stdout == (correcting ? native(logical) : ''.b)

    logical_tail = nil
    safe = stdout
    if correcting
      # 元 fixture が記録した補正と、既存の物理 CRLF 保持を組み合わせる。
      physical = before.include?("\r\n") ? native(logical.gsub("\r\n".b, "\n".b)) : logical
      logical_tail = tail_start(logical)
      physical_tail = tail_start(physical)
      raise "tail classification changed at #{index}" unless logical_tail.nil? == physical_tail.nil?
      safe = native(logical.byteslice(0, logical_tail)) + physical.byteslice(physical_tail..) if logical_tail
    end
    measured[index.to_s] = {
      name: entry.fetch('name'), ruby: entry.fetch('ruby'), eol: entry.fetch('eol'), mode: entry.fetch('mode'),
      source_sha256: Digest::SHA256.hexdigest(before), exit: status,
      source_hex: entry.fetch('source_hex'),
      upstream_stdout_hex: stdout.unpack1('H*'), upstream_stderr_hex: stderr.unpack1('H*'),
      protected_tail: !logical_tail.nil?, expected_stdout_hex: safe.unpack1('H*')
    }
  end
end
raise 'missing measurements' unless measured.length == records.length
result = {
  reference: { ruby: RUBY_VERSION, platform: RUBY_PLATFORM, gems: %w[parser prism rubocop-ast rubocop].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] } },
  source_fixture: File.basename(source_path), source_fixture_sha256: Digest::SHA256.hexdigest(source_bytes),
  cases: measured
}
File.binwrite(output_path, JSON.pretty_generate(result) + "\n")
puts "recorded #{measured.length} complete CLI runs"
