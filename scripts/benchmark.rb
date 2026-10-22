#!/usr/bin/env ruby
# frozen_string_literal: true

require 'digest'
require 'etc'
require 'fileutils'
require 'json'
require 'open3'
require 'optparse'
require 'rbconfig'
require 'time'
require 'tmpdir'

module SonicopBenchmark
  # CONFORMANCE.md と同じ木を使い、コーパス側の更新を性能変化へ混ぜない。
  CORPORA = {
    'rails' => ['rails/rails', '62b545861ef3edf472149504d92631ff7f88804f'],
    'rubocop' => ['rubocop/rubocop', 'f009b338048ea0ade9d6d88cf358111092c8f12c'],
    'mastodon' => ['mastodon/mastodon', 'fad36853ce17045829040f8f76506dfb51d773f2'],
    'homebrew' => ['Homebrew/brew', '38ee3253f5920e4a533c8f956bedb3ae96b12a9c'],
    'ruby' => ['ruby/ruby', '3349f4107d268658fdf8cc6b979fbb1c923e597f']
  }.freeze
  RUBY_EXCLUSIONS = %w[test/ruby/test_regexp.rb test/ruby/test_file_exhaustive.rb].freeze
  REFERENCE_VERSION = '1.92.1'
  # 固定コーパスを親 gemspec の無い場所で実行した本家 CLI の実効値。
  TARGET_RUBY_VERSIONS = { 'rails' => '3.3', 'rubocop' => '2.7', 'mastodon' => '4.0',
                           'homebrew' => '2.7', 'ruby' => '2.7' }.freeze

  class Report
    attr_reader :paths, :counts, :offenses

    def initialize(document)
      files = document.fetch('files')
      summary = document.fetch('summary')
      count = summary.fetch('inspected_file_count')
      unless files.is_a?(Array) && count.is_a?(Integer) && count.positive? &&
             count == summary.fetch('target_file_count') && count == files.length
        raise ArgumentError, 'incomplete inspection'
      end

      @counts = Hash.new(0)
      @offenses = Hash.new(0)
      @paths = files.map do |file|
        path = file.fetch('path').delete_prefix('./')
        raise ArgumentError, 'invalid offense list' unless file.fetch('offenses').is_a?(Array)

        file.fetch('offenses').each do |offense|
          location = offense.fetch('location')
          unless %w[cop_name message severity].all? { |key| offense[key].is_a?(String) } &&
                 %w[line column last_line last_column length].all? { |key| location[key].is_a?(Integer) } &&
                 [true, false].include?(offense['correctable'])
            raise ArgumentError, 'invalid offense fields'
          end
          cop = offense.fetch('cop_name')
          @counts[cop] += 1
          @offenses[[path, cop, location, offense.fetch('message'), offense.fetch('severity'),
                     offense.fetch('correctable')]] += 1
        end
        path
      end.sort
      raise ArgumentError, 'duplicate target paths' unless @paths.uniq.length == @paths.length
      raise ArgumentError, 'inconsistent offense count' unless @counts.values.sum == summary.fetch('offense_count')
    end

    def compare(other)
      raise ArgumentError, 'different target paths' unless paths == other.paths
      raise ArgumentError, 'no exercised cops' if counts.empty? && other.counts.empty?

      # 同じ位置の別メッセージや重複も残し、過剰と不足を総数で相殺しない。
      keys = offenses.keys | other.offenses.keys
      {
        'reference_only' => keys.sum { |key| [offenses[key] - other.offenses[key], 0].max },
        'candidate_only' => keys.sum { |key| [other.offenses[key] - offenses[key], 0].max }
      }
    end
  end

  def self.enabled_cops(text)
    cops = text.split(/^(?=[A-Z][A-Za-z]*\/[A-Za-z0-9_]+:$)/).filter_map do |section|
      name = section[/\A([A-Z][A-Za-z]*\/[A-Za-z0-9_]+):$/, 1]
      name if name && section.match?(/^  Enabled: true$/)
    end
    raise ArgumentError, 'no enabled cops' if cops.empty?
    raise ArgumentError, 'duplicate cop names' unless cops.uniq.length == cops.length

    cops.sort
  end

  class Runner
    def initialize(candidate:, reference:, output:, repeats:)
      raise ArgumentError, 'use two or three repetitions' unless [2, 3].include?(repeats)

      @candidate = File.realpath(candidate)
      @reference = reference
      @output = File.expand_path(output)
      raise ArgumentError, 'output directory must be empty' if Dir.exist?(@output) && !Dir.empty?(@output)

      FileUtils.mkdir_p(@output)
      @output = File.realpath(@output)
      @repeats = repeats
      # 引数ファイルと同じく、呼び出し元の環境も --force-default-config だけでは消えない。
      @environment = ENV.to_h.select do |key, _value|
        %w[PATH HOME TMPDIR GEM_HOME GEM_PATH LANG LC_ALL SSL_CERT_FILE SSL_CERT_DIR].include?(key)
      end.merge('RUBYOPT' => '--yjit', 'RAYON_NUM_THREADS' => Etc.nprocessors.to_s,
                'PARALLEL_PROCESSOR_COUNT' => Etc.nprocessors.to_s,
                'XDG_CACHE_HOME' => File.join(@output, 'environment-cache'))
    end

    def capture(*command, chdir: @output)
      stdout, stderr, status = Open3.capture3(@environment, *command, chdir: chdir, unsetenv_others: true)
      raise "command failed (#{status.exitstatus}): #{command.first}\n#{stderr}" unless status.success?

      stdout.strip
    end

    def load_average
      if File.file?('/proc/loadavg')
        File.read('/proc/loadavg').split.first(3).map(&:to_f)
      elsif RbConfig::CONFIG.fetch('host_os').include?('darwin')
        capture('sysctl', '-n', 'vm.loadavg').scan(/[\d.]+/).map(&:to_f)
      end
    end

    def process(command, tree, path)
      before_load = load_average
      before = Process.times
      start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      status = File.open(path, 'wb') do |stdout|
        File.open("#{path}.err", 'wb') do |stderr|
          Process.wait2(Process.spawn(@environment, *command, chdir: tree, out: stdout, err: stderr,
                                     unsetenv_others: true)).last
        end
      end
      elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - start
      after = Process.times
      record = { 'real' => elapsed, 'user' => after.cutime - before.cutime,
                 'system' => after.cstime - before.cstime, 'exit' => status.exitstatus,
                 'load_before' => before_load, 'load_after' => load_average }
      File.write("#{path}.run.json", JSON.pretty_generate(record))
      raise "inspection failed: #{path} (#{status.exitstatus})" unless [0, 1].include?(status.exitstatus)
      if File.read("#{path}.err").match?(/An error occurred while|\b\d+ errors? occurred\b/)
        raise "internal cop error: #{path}.err"
      end

      record
    end

    def verify_run(path, expected)
      actual = Report.new(JSON.parse(File.read(path)))
      unless actual.paths == expected.paths && actual.offenses == expected.offenses
        raise "timed inspection changed: #{path}"
      end

      { 'inspected_files' => actual.paths.length, 'offenses' => actual.counts.values.sum,
        'report_sha256' => Digest::SHA256.file(path).hexdigest }
    end

    def target_versions(name, tree, commands)
      versions = commands.to_h do |side, command|
        stdout, stderr, status = Open3.capture3(
          @environment, *command, '--stdin', 'sonicop-target-ruby-probe.rb', '-f', 'json',
          stdin_data: "def probe(\n", chdir: tree, unsetenv_others: true
        )
        raise "target-version probe failed: #{side}\n#{stderr}" unless status.exitstatus == 1

        report = Report.new(JSON.parse(stdout))
        # 診断の source/version は独自推定せず、実際のパーサのメッセージから取る。
        message = report.offenses.keys.find { |entry| entry[1] == 'Lint/Syntax' }&.[](3)
        version = message&.match(/Using Ruby ([\d.]+) parser/)&.captures&.first
        raise "unreadable target-version probe: #{side}" unless version

        [side, version]
      end
      expected = TARGET_RUBY_VERSIONS.fetch(name)
      unless versions.values.all? { |version| version == expected }
        raise "unexpected TargetRubyVersion for #{name}: expected #{expected}, got #{versions.inspect}"
      end

      versions
    end

    def run
      reference_version = capture(@reference, '--version')
      unless reference_version == REFERENCE_VERSION
        raise "expected RuboCop #{REFERENCE_VERSION}, got #{reference_version}"
      end

      cpu = if File.file?('/proc/cpuinfo')
              File.read('/proc/cpuinfo')[/^model name\s*:\s*(.+)$/, 1]
            else
              capture('sysctl', '-n', 'machdep.cpu.brand_string')
            end
      document = { 'measured_at' => Time.now.utc.iso8601, 'ruby' => RUBY_DESCRIPTION,
                   'reference' => reference_version,
                   'candidate' => capture(@candidate, '--version'),
                   'binary_sha256' => Digest::SHA256.file(@candidate).hexdigest,
                   'revision' => capture('git', '-C', File.expand_path('..', __dir__), 'rev-parse', 'HEAD'),
                   'host' => RbConfig::CONFIG.fetch('host'), 'cpu' => cpu, 'cores' => Etc.nprocessors,
                   'rubyopt' => @environment.fetch('RUBYOPT'), 'runner_image' => ENV['ImageVersion'],
                   'runs' => [] }
      document['reference_gems'] = JSON.parse(capture(
        RbConfig.ruby, '-rjson', '-e',
        'gem "rubocop", ARGV.fetch(0); require "rubocop"; puts Gem.loaded_specs.transform_values { |spec| spec.version.to_s }.to_json',
        reference_version
      ))
      commands = { 'reference' => [@reference, '--force-default-config'],
                   'candidate' => [@candidate, '--force-default-config'] }
      enabled = commands.to_h do |side, command|
        text = capture(*command, '--show-cops')
        File.write(File.join(@output, "#{side}-cops.yml"), text)
        [side, SonicopBenchmark.enabled_cops(text)]
      end
      common = enabled.fetch('reference') & enabled.fetch('candidate')
      raise ArgumentError, 'no common enabled cops' if common.empty?

      document['enabled_cops'] = common
      document['excluded_cops'] = enabled.transform_values { |names| names - common }
      commands.each do |side, command|
        excluded = document.fetch('excluded_cops').fetch(side)
        # --only は directive 系 cop の意味を変えるため、既定集合から差だけを除く。
        command.concat(['--except', excluded.join(',')]) unless excluded.empty?
      end
      persist(document)
      # 出力をリポジトリ内に保存しても、入力が親の sonicop.gemspec を継承しないようにする。
      source_root = Dir.mktmpdir('sonicop-benchmark-', ENV.fetch('RUNNER_TEMP', Dir.tmpdir))
      CORPORA.each do |name, (repository, sha)|
        tree = File.join(source_root, name)
        FileUtils.mkdir_p(tree)
        capture('git', 'init', '--quiet', tree)
        capture('git', '-C', tree, 'fetch', '--quiet', '--depth=1', "https://github.com/#{repository}.git", sha)
        capture('git', '-C', tree, 'checkout', '--quiet', '--detach', 'FETCH_HEAD')
        raise 'corpus revision changed' unless capture('git', '-C', tree, 'rev-parse', 'HEAD') == sha

        exclusions = name == 'ruby' ? RUBY_EXCLUSIONS.dup : []
        exclusions << '.rubocop' if File.file?(File.join(tree, '.rubocop'))
        # 本家 parser が中断する既知入力だけを複製から除き、対象集合を両者で揃える。
        exclusions.each { |path| File.unlink(File.join(tree, path)) }
        row = measure(name, tree, commands)
        row.merge!('repository' => repository, 'sha' => sha, 'excluded_paths' => exclusions)
        document.fetch('runs') << row
        persist(document)
      end
      document
    end

    def measure(name, tree, commands)
      out = File.join(@output, name)
      FileUtils.mkdir_p(out)
      versions = target_versions(name, tree, commands)
      paths = commands.to_h do |side, command|
        path = File.join(out, "#{side}-paths.txt")
        process(command + ['--cache', 'false', '-L', '.'], tree, path)
        [side, File.readlines(path, chomp: true).map { |line| line.delete_prefix('./') }.sort]
      end
      unless paths.values.all? { |list| !list.empty? && list.uniq == list } && paths.values.uniq.length == 1
        raise 'different or empty discovery paths'
      end
      reports = commands.to_h do |side, command|
        path = File.join(out, "#{side}.json")
        process(command + ['--cache', 'false', '-f', 'json', '.'], tree, path)
        report = Report.new(JSON.parse(File.read(path)))
        raise 'inspection and discovery paths differ' unless report.paths == paths.fetch(side)

        [side, report]
      end
      row = { 'name' => name, 'files' => paths.values.first.length,
              'target_ruby_versions' => versions,
              'diagnostic_difference' => reports.fetch('reference').compare(reports.fetch('candidate')),
              'offenses' => reports.transform_values { |report| { 'total' => report.counts.values.sum, 'by_cop' => report.counts } },
              'conditions' => {} }
      %w[parallel single].each do |condition|
        @repeats.times do |number|
          # 偶数回と奇数回で先行側を替え、常に後から走る側だけが温まることを避ける。
          order = number.even? ? commands : commands.to_a.reverse.to_h
          order.each do |side, command|
            cache = File.join(out, "cache-#{side}-#{number}")
            suffix = if condition == 'parallel'
                       Dir.mkdir(cache)
                       ['--parallel', '--cache', 'true', '--cache-root', cache]
                     else
                       (side == 'candidate' ? ['--no-parallel'] : []) + ['--cache', 'false']
                     end
            # 計測する実行そのものの JSON を照合する。事前検査だけでは途中中断を検出できない。
            path = File.join(out, "#{condition}-#{side}-#{number}.json")
            record = process(command + suffix + ['--no-color', '-f', 'json', '.'], tree, path)
            record.merge!(verify_run(path, reports.fetch(side)))
            record['cache_files'] = Dir.glob(File.join(cache, '**', '*')).count { |file| File.file?(file) } if condition == 'parallel'
            File.write("#{path}.run.json", JSON.pretty_generate(record))
            (row.fetch('conditions')["#{condition}-#{side}"] ||= []) << record
            puts [name, condition, side, number, record].inspect
            $stdout.flush
          end
        end
      end
      row
    end

    def persist(document)
      File.write(File.join(@output, 'results.json'), JSON.pretty_generate(document))
    end
  end

  def self.main(argv)
    options = { candidate: 'target/release/sonicop', reference: 'rubocop', output: 'tmp/benchmark', repeats: 2 }
    OptionParser.new do |parser|
      parser.on('--candidate PATH') { |value| options[:candidate] = value }
      parser.on('--reference PATH') { |value| options[:reference] = value }
      parser.on('--output PATH') { |value| options[:output] = value }
      parser.on('--repeats N', Integer) { |value| options[:repeats] = value }
    end.parse!(argv)
    raise ArgumentError, 'unexpected arguments' unless argv.empty?

    Runner.new(**options).run
  end
end

SonicopBenchmark.main(ARGV) if $PROGRAM_NAME == __FILE__
