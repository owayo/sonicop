# frozen_string_literal: true

require 'digest'
require_relative 'record_external_options'

# 採取器を require しても47件を再採取せず、同じ子起動経路だけを検査する。
module ExternalOptionsEnvironmentTest
  POISON = {
    'RUBYOPT' => '-rsonicop_env4_missing_require',
    'RUBYLIB' => 'sonicop_env4_unexpected_load_path',
    'RUBOCOP_OPTS' => '--only Unknown/Cop',
    'RUBOCOP_TARGET_RUBY_VERSION' => '0.0'
  }.freeze
  PRESERVED = %w[GEM_HOME GEM_PATH PATH SystemRoot OS SONICOP_ENV_PARENT_SENTINEL].freeze
  PROBE = <<~'RUBY'
    STDOUT.binmode
    keys = %w[RUBYOPT RUBYLIB RUBOCOP_OPTS RUBOCOP_TARGET_RUBY_VERSION GEM_HOME GEM_PATH PATH SystemRoot OS SONICOP_ENV_PARENT_SENTINEL] + ARGV
    STDOUT.write(Marshal.dump(keys.to_h { |key| [key, ENV[key]] }))
  RUBY

  def self.check(condition, message)
    raise message unless condition
  end

  def self.with_poison(poison = POISON)
    original = ENV.to_h
    ExternalOptionsReference::ENV_KEYS.each { |key| ENV.delete(key) }
    ENV.update(poison.merge('SONICOP_ENV_PARENT_SENTINEL' => '親環境を保持する'))
    begin
      poisoned_parent = ENV.to_h
      yield poisoned_parent
      check(ENV.to_h == poisoned_parent, '子の環境分離が親ENVを変更した')
    ensure
      ENV.replace(original)
    end
    check(ENV.to_h == original, '試験後に親ENVが戻らない')
  end

  def self.core_contract(case_options)
    with_poison do |parent|
      stdout, stderr, status = ExternalOptionsReference.capture(
        case_options, RbConfig.ruby, '--disable-gems', '-e', PROBE, binmode: true
      )
      check(status.success? && stderr.empty?, '環境poisonがcore子Rubyへ漏れた')
      observed = Marshal.load(stdout)
      expected = ExternalOptionsReference::ENV_KEYS.to_h { |key| [key, nil] }
      expected['RUBOCOP_OPTS'] = case_options unless case_options.nil?
      # Windows ENV のキー検索は大文字小文字を区別しないが、to_h の通常Hashは区別する。
      PRESERVED.each { |key| expected[key] = ENV[key] }
      mismatches = expected.keys.select { |key| observed[key] != expected[key] }
      check(observed == expected, "4キー除去・case上書き・探索環境保持の契約が違う: #{mismatches.join(',')}")
    end
  end

  def self.mixed_case_contract
    variants = %w[rUbYoPt RuByLiB rubocop_opts RuBoCoP_tArGeT_rUbY_vErSiOn]
    poison = variants.zip(POISON.values).to_h
    keep_name = 'sOnIcOp_CaSe_KeEp'
    with_poison(poison.merge(keep_name => '名前のspellingも保持する')) do
      environment = ExternalOptionsReference.child_environment(nil)
      check(environment.key?(keep_name), '無関係なENV名のspellingを変えた')
      stdout, stderr, status = ExternalOptionsReference.capture(
        nil, RbConfig.ruby, '--disable-gems', '-e', PROBE, *variants, keep_name, binmode: true
      )
      check(status.success? && stderr.empty?, '混在caseのstartup poisonが子へ漏れた')
      observed = Marshal.load(stdout)
      ExternalOptionsReference::ENV_KEYS.each { |key| check(observed[key].nil?, '標準4名が残った') }
      variants.each do |key|
        # Unixではcaseの異なる名は別物なので、除去せず元の値を保持する。
        expected = File::ALT_SEPARATOR == '\\' ? nil : poison.fetch(key)
        check(observed[key] == expected, 'OSごとのENV名case規則を変えた')
      end
      check(observed[keep_name] == ENV[keep_name], '無関係な親の値を変えた')
      PRESERVED.each { |key| check(observed[key] == ENV[key], '探索環境を変えた') }
    end
  end

  def self.reference_controls(poisoned:)
    root = File.expand_path('..', __dir__)
    paths = %w[external_options_windows.json shellwords_options.json].map do |name|
      File.join(root, 'tests', 'fixtures', name)
    end
    before = paths.to_h { |path| [path, Digest::SHA256.file(path).hexdigest] }
    fixture = JSON.parse(File.binread(paths[0]))
    tokens = JSON.parse(File.binread(paths[1]))
    check(fixture.fetch('cases').length == 47, '旧47CLI条件の数が変わった')
    check(tokens.fetch('cases').length == 43, '旧43token条件の数が変わった')
    selected = fixture.fetch('cases').select do |entry|
      %w[dotfile-vt-delimiter environment-vt-delimiter].include?(entry.fetch('id'))
    end
    check(selected.length == 2, '代表2条件が揃っていない')
    loader = 'gem "parser", "3.3.12.0"; gem "rubocop", "1.89.0"; load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
    results = []
    capture_cases = proc do
      selected.each do |entry|
        Dir.mktmpdir('sonicop-external-options-env-') do |directory|
          File.binwrite(File.join(directory, 'config.yml'), "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n")
          source = [entry.fetch('options_hex')].pack('H*')
          environment_options = entry.fetch('origin') == 'environment' ? source : nil
          File.binwrite(File.join(directory, '.rubocop'), source) if entry.fetch('origin') == 'dotfile'
          stdout, stderr, status = ExternalOptionsReference.capture(
            environment_options, RbConfig.ruby, '-e', loader, '--', *entry.fetch('arguments'),
            stdin_data: [entry.fetch('stdin_hex')].pack('H*'), chdir: directory, binmode: true
          )
          report = begin
            JSON.parse(stdout).slice('files', 'summary')
          rescue JSON::ParserError
            nil
          end
          check(status.exitstatus == entry.fetch('exit'), '代表CLIの終了値が変わった')
          check(report == entry.fetch('report'), '代表CLIの診断全項目が変わった')
          check(stdout.empty? == entry.fetch('stdout_empty'), '代表CLIのstdout空判定が変わった')
          check(stderr.empty? == entry.fetch('stderr_empty'), '代表CLIのstderr空判定が変わった')
          check(!File.exist?(File.join(directory, 'example.rb')), 'stdinがディスク原本を作った')
          metadata = JSON.parse(stdout).fetch('metadata')
          check(metadata.fetch('rubocop_version') == '1.89.0' && metadata.fetch('ruby_version') == '4.0.7', '実参照版が違う')
          results << { id: entry.fetch('id'), poisoned: poisoned, exit: status.exitstatus,
                       stdout_hex: stdout.unpack1('H*'), stderr_hex: stderr.unpack1('H*'),
                       report: report, fixture_full_record_equal: true }
        end
      end
    end
    poisoned ? with_poison(&capture_cases) : capture_cases.call
    check(paths.all? { |path| Digest::SHA256.file(path).hexdigest == before[path] }, '既存fixtureを変更した')
    results
  end

  def self.run(actual_reference: false)
    core_contract(nil)
    core_contract('--only Layout/SpaceAroundOperators')
    core_contract('')
    mixed_case_contract
    results = actual_reference ? reference_controls(poisoned: false) + reference_controls(poisoned: true) : []
    puts JSON.generate(core_child_contracts: 4, mixed_ascii_case_contracts: 1, actual_reference_conditions: actual_reference ? 2 : 0,
                       actual_reference_child_runs: results.length, reference_results: results,
                       parent_preserved: true, gem_path_os_preserved: true)
  end
end

ExternalOptionsEnvironmentTest.run(actual_reference: ARGV == ['--actual-reference']) if $PROGRAM_NAME == __FILE__
