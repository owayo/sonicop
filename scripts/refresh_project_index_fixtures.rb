#!/usr/bin/env ruby
# frozen_string_literal: true

# 入力と索引対象を fixture に残し、期待値は必ず固定版の本家 CLI から作り直す。
require 'fileutils'
require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'

gem 'rubocop', '1.89.0'
gem 'rubydex', '0.4.1'
require 'rubocop'
require 'rubydex'

%w[project_index_cases.json path_cop_cases.json].each do |filename|
  fixture = File.expand_path("../tests/fixtures/#{filename}", __dir__)
  cases = JSON.parse(File.read(fixture))
  cases.each_with_index do |entry, number|
    Dir.mktmpdir('project-index-fixture-') do |directory|
      entry.fetch('files').each do |path, source|
        destination = File.join(directory, path)
        FileUtils.mkdir_p(File.dirname(destination))
        File.binwrite(destination, source)
      end
      File.write(File.join(directory, '.rubocop.yml'), JSON.generate(entry.fetch('config')))
      bootstrap = 'gem "rubocop", "1.89.0"; gem "rubydex", "0.4.1"; load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
      targets = entry.key?('stdin') ? ['--stdin', entry.fetch('targets').fetch(0)] : entry.fetch('targets')
      output, error, status = Open3.capture3(
        RbConfig.ruby, '-e', bootstrap, '--', '--cache', 'false', '--config', '.rubocop.yml',
        '--only', entry.fetch('cop'), '--format', 'json', *targets, chdir: directory,
        stdin_data: entry.fetch('stdin', '')
      )
      abort "case #{number}: exit #{status.exitstatus}: #{error}" unless [0, 1].include?(status.exitstatus)

      report = JSON.parse(output)
      expected_paths = entry.fetch('targets').sort
      unless report.fetch('summary').fetch('inspected_file_count') == expected_paths.length &&
             report.fetch('summary').fetch('target_file_count') == expected_paths.length &&
             report.fetch('files').map { |file| file.fetch('path') }.sort == expected_paths
        abort "case #{number}: incomplete inspection"
      end
      entry['expected'] = report.fetch('files')
      entry['expected'].each do |file|
        file.fetch('offenses').each do |offense|
          offense['message'] = offense.fetch('message').gsub(File.realpath(directory), '{root}').gsub(directory, '{root}')
        end
      end
      warn "#{number + 1}/#{cases.length}: #{entry.fetch('cop')}"
    end
  end
  File.write(fixture, "#{JSON.pretty_generate(cases)}\n")
end
