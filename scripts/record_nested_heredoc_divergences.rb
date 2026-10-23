# frozen_string_literal: true

# parser gem が入れ子の heredoc を重複して読む実出力を記録する。候補の出力は期待値に使わない。
require 'digest'
require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'

ROOT = File.expand_path('..', __dir__)
FIXTURE = File.join(ROOT, 'tests/fixtures/nested_heredoc_divergences.json')
COPS = %w[Metrics/AbcSize Metrics/CyclomaticComplexity Metrics/PerceivedComplexity Lint/UselessAssignment].freeze
RUBOCOP = Gem.bin_path('rubocop', 'rubocop', '1.89.0')
MODES = %w[inspect -a -A].freeze
ENDINGS = { 'lf' => "\n", 'crlf' => "\r\n" }.freeze

# Ruby の実行と構文検査は同じバイト列で一度だけ行い、版・engine・mode ごとの記録を共有する。
def execution(source, run, cache)
  cache[[source, run]] ||= Dir.mktmpdir('sonicop-heredoc-runtime-') do |dir|
    path = File.join(dir, 'example.rb')
    File.binwrite(path, source)
    _out, err, status = Open3.capture3(RbConfig.ruby, '-c', path)
    raise "構文検査が失敗: #{err}" unless status.success?

    result = { 'syntax_valid' => true }
    if run
      out, err, status = Open3.capture3(RbConfig.ruby, path)
      result.merge!('exit' => status.exitstatus, 'stdout' => out,
                    'error_class' => err[/\((\w+(?:::\w+)*)\)/, 1])
    end
    result
  end
end

def config(profile)
  "AllCops:\n  TargetRubyVersion: #{profile.fetch('target')}\n" \
    "  ParserEngine: #{profile.fetch('engine')}\n  NewCops: enable\n" +
    COPS.map { |cop| "#{cop}:\n  Enabled: true\n#{cop.start_with?('Metrics/') ? "  Max: 0\n" : ''}" }.join
end

fixture = JSON.parse(File.binread(FIXTURE))
raise 'RuboCop 1.89.0 が必要' unless Gem::Specification.find_by_name('rubocop', '1.89.0').version.to_s == '1.89.0'
fixture['reference'] = "RuboCop 1.89.0 / Ruby #{RUBY_VERSION}"
fixture['only'] = COPS.join(',')
fixture['records'] = {}
fixture['unsupported'] = {}
runtime_cache = {}
count = 0
fixture.fetch('profiles').each { |profile| profile['yaml'] = config(profile) }
fixture.fetch('cases').each_with_index do |item, item_index|
  scopes = [['reference', fixture.fetch('only')]]
  scopes << ['variable_reference', 'Lint/UselessAssignment'] if item.fetch('id') == 'used_assignment'
  scopes.each do |field, only|
    item[field] = {}
    fixture.fetch('profiles').each do |profile|
      effective = profile.fetch('effective')
      next if effective.nil? && item_index.positive?

      ENDINGS.each do |ending_name, ending|
        source = item.fetch('source').gsub("\n", ending)
        MODES.each do |mode|
          record = Dir.mktmpdir('sonicop-heredoc-reference-') do |dir|
            File.binwrite(File.join(dir, 'example.rb'), source)
            File.binwrite(File.join(dir, '.rubocop.yml'), profile.fetch('yaml'))
            cmd = [RbConfig.ruby, RUBOCOP, '--cache', 'false', '--only', only, '--format', 'json']
            cmd << mode unless mode == 'inspect'
            out, err, status = Open3.capture3(*cmd, 'example.rb', chdir: dir)
            after = File.binread(File.join(dir, 'example.rb')).force_encoding(Encoding::UTF_8)
            if effective.nil?
              error = err.lines.find { |line| line.include?('RuboCop supports target Ruby versions') }&.strip
              raise "想定外の設定拒否: #{err}" unless status.exitstatus == 2 && error && after == source

              { 'exit' => 2, 'error' => error, 'after' => after }
            else
              data = JSON.parse(out)
              raise "未完了の検査: #{err}" unless data.dig('summary', 'inspected_file_count') == 1 && data.dig('summary', 'target_file_count') == 1

              { 'exit' => status.exitstatus, 'offenses' => data.fetch('files').fetch(0).fetch('offenses'),
                'after' => after, 'execution' => execution(after, item.fetch('run'), runtime_cache) }
            end
          end
          key = Digest::SHA256.hexdigest(JSON.generate(record))[0, 16]
          fixture['records'][key] = record
          target = if effective
                     item[field][effective] ||= {}
                   else
                     fixture['unsupported']
                   end
          target[ending_name] ||= {}
          previous = target[ending_name][mode]
          raise "版・engine で予期しない差: #{item.fetch('id')} #{profile.inspect} #{ending_name} #{mode}" if previous && previous != key

          target[ending_name][mode] = key
          count += 1
        end
      end
    end
  end
  warn "#{item.fetch('id')}: #{count} 実行"
end
File.binwrite(FIXTURE, JSON.pretty_generate(fixture) + "\n")
warn "#{count} 実行 / #{fixture.fetch('records').size} 記録"
