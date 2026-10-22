#!/usr/bin/env ruby
# frozen_string_literal: true

require 'fileutils'
require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'

gem 'rubocop', '1.89.0'
require 'rubocop'

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
    targets = { 'absolute' => File.expand_path(file), 'canonical_absolute' => File.realpath(file),
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
        rows << { 'kind' => kind, 'control' => control, 'cwd' => Dir.pwd, 'canonical_cwd' => canonical,
                  'target' => target, 'exit' => status.exitstatus, 'report' => report }
      end
    end
  end
end
puts JSON.pretty_generate('ruby' => RUBY_DESCRIPTION, 'reference' => RuboCop::Version::STRING, 'runs' => rows)
