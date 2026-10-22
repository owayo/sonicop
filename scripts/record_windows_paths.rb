#!/usr/bin/env ruby
# frozen_string_literal: true

require 'fileutils'
require 'fiddle'
require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'

gem 'rubocop', '1.89.0'
require 'rubocop'

# Ruby の realpath は Windows の 8.3 名を展開しないため、Rust の canonicalize と
# 同じ長い名前の入力を作るには Win32 の照会を別に使う。
def reference_canonical_path(path)
  return File.realpath(path) unless Gem.win_platform?

  capacity = 32_768
  handle = Fiddle.dlopen('kernel32.dll')
  function = Fiddle::Function.new(handle['GetLongPathNameW'],
                                  [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP, Fiddle::TYPE_LONG], Fiddle::TYPE_LONG)
  input = path.encode(Encoding::UTF_16LE).b + "\0\0".b
  output = "\0".b * (capacity * 2)
  length = function.call(input, output, capacity)
  raise 'GetLongPathNameW failed' unless length.positive? && length < capacity

  output.byteslice(0, length * 2).force_encoding(Encoding::UTF_16LE).encode(Encoding::UTF_8).tr('\\', '/')
end

source = "class Foo\n  def bar; end\n\n  def bar; end\nend\n"
quiet = "class Foo\n  def bar; end\nend\n"
rows = []
Dir.mktmpdir('path-reference-') do |directory|
  FileUtils.mkdir_p(File.join(directory, 'nested/lib'))
  File.write(File.join(directory, 'nested/.rubocop.yml'), "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n")
  file = File.join(directory, 'nested/lib/probe.rb')
  File.write(file, source)
  canonical = File.realpath(directory)
  Dir.chdir(directory) do
    targets = { 'absolute' => File.expand_path(file), 'canonical_absolute' => reference_canonical_path(file),
                'relative' => 'nested/lib/probe.rb' }
    targets.each do |kind, target|
      [source, quiet].each_with_index do |input, control|
        args = [RbConfig.ruby, Gem.bin_path('rubocop', 'rubocop', '1.89.0'),
                '--cache', 'false', '--only', 'Lint/DuplicateMethods', '-f', 'json', '--stdin', target]
        output, error, status = Open3.capture3(*args, stdin_data: input)
        report = JSON.parse(output)
        expected = control.zero? ? 1 : 0
        raise error unless status.exitstatus == expected && report.dig('summary', 'offense_count') == expected
        raise 'incomplete inspection' unless report.dig('summary', 'inspected_file_count') == 1

        # 短い作業パスと長い入力パスを同一視するかは、本家自身の出力から判断する。
        row = { 'kind' => kind, 'control' => control, 'cwd' => Dir.pwd, 'canonical_cwd' => canonical,
                  'target' => target, 'exit' => status.exitstatus, 'report' => report }
        if ENV['SONICOP_BINARY']
          candidate_args = [ENV.fetch('SONICOP_BINARY'), *args.drop(2)]
          candidate_output, candidate_error, candidate_status = Open3.capture3(*candidate_args, stdin_data: input)
          candidate = JSON.parse(candidate_output)
          raise candidate_error unless candidate_status.exitstatus == expected && candidate.dig('summary', 'offense_count') == expected

          row['candidate'] = candidate
          row['matches'] = candidate.fetch('files') == report.fetch('files')
        end
        rows << row
      end
    end
    args = [RbConfig.ruby, Gem.bin_path('rubocop', 'rubocop', '1.89.0'),
            '--cache', 'false', '--only', 'Lint/DuplicateMethods', '-f', 'json', '.']
    output, error, status = Open3.capture3(*args)
    report = JSON.parse(output)
    raise error unless status.exitstatus == 1 && report.dig('summary', 'offense_count') == 1

    row = { 'kind' => 'discovery', 'cwd' => Dir.pwd, 'report' => report }
    if ENV['SONICOP_BINARY']
      candidate_output, candidate_error, candidate_status = Open3.capture3(ENV.fetch('SONICOP_BINARY'), *args.drop(2))
      candidate = JSON.parse(candidate_output)
      raise candidate_error unless candidate_status.exitstatus == 1 && candidate.dig('summary', 'offense_count') == 1

      row['candidate'] = candidate
      row['matches'] = candidate.fetch('files') == report.fetch('files')
    end
    rows << row
  end
end
puts JSON.pretty_generate('ruby' => RUBY_DESCRIPTION, 'reference' => RuboCop::Version::STRING, 'runs' => rows)
