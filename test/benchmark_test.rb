# frozen_string_literal: true

require 'test_helper'
require_relative '../scripts/benchmark'

class BenchmarkTest < Minitest::Test
  def document
    { 'files' => [{ 'path' => 'example.rb', 'offenses' => [
      { 'cop_name' => 'Style/Example', 'message' => 'Example.', 'severity' => 'convention', 'correctable' => true,
        'location' => { 'line' => 1, 'column' => 1, 'last_line' => 1, 'last_column' => 2, 'length' => 2 } }
    ] }], 'summary' => { 'target_file_count' => 1, 'inspected_file_count' => 1, 'offense_count' => 1 } }
  end

  def test_complete_identical_report_matches
    report = SonicopBenchmark::Report.new(document)
    assert_equal({ 'reference_only' => 0, 'candidate_only' => 0 }, report.compare(SonicopBenchmark::Report.new(document)))
  end

  def test_end_column_difference_is_preserved
    changed = document
    changed.fetch('files').first.fetch('offenses').first.fetch('location')['last_column'] = 3
    assert_equal({ 'reference_only' => 1, 'candidate_only' => 1 },
                 SonicopBenchmark::Report.new(document).compare(SonicopBenchmark::Report.new(changed)))
  end

  def test_incomplete_inspection_is_rejected
    changed = document
    changed.fetch('summary')['target_file_count'] = 2
    assert_raises(ArgumentError) { SonicopBenchmark::Report.new(changed) }
  end

  def test_different_paths_are_rejected
    changed = document
    changed.fetch('files').first['path'] = 'other.rb'
    assert_raises(ArgumentError) { SonicopBenchmark::Report.new(document).compare(SonicopBenchmark::Report.new(changed)) }
  end

  def test_duplicate_offenses_are_not_collapsed
    changed = document
    changed.fetch('files').first.fetch('offenses') << changed.fetch('files').first.fetch('offenses').first.dup
    changed.fetch('summary')['offense_count'] = 2
    assert_equal({ 'reference_only' => 0, 'candidate_only' => 1 },
                 SonicopBenchmark::Report.new(document).compare(SonicopBenchmark::Report.new(changed)))
  end

  def test_enabled_cop_catalog_excludes_pending_and_disabled
    catalog = "Style/Enabled:\n  Enabled: true\nStyle/Pending:\n  Enabled: pending\nStyle/Disabled:\n  Enabled: false\n"
    assert_equal ['Style/Enabled'], SonicopBenchmark.enabled_cops(catalog)
  end

  def test_two_silent_reports_are_unmeasured
    changed = document
    changed.fetch('files').first['offenses'] = []
    changed.fetch('summary')['offense_count'] = 0
    report = SonicopBenchmark::Report.new(changed)
    assert_raises(ArgumentError) { report.compare(report) }
  end

  def test_duplicate_target_paths_are_rejected
    changed = document
    changed.fetch('files') << changed.fetch('files').first.dup
    changed.fetch('summary').merge!('target_file_count' => 2, 'inspected_file_count' => 2, 'offense_count' => 2)
    assert_raises(ArgumentError) { SonicopBenchmark::Report.new(changed) }
  end

  def test_timed_empty_output_is_rejected_even_with_exit_zero
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      path = File.join(root, 'report.json')
      runner.process([RbConfig.ruby, '-e', 'exit 0'], root, path)
      assert_raises(JSON::ParserError) { runner.verify_run(path, SonicopBenchmark::Report.new(document)) }
    end
  end

  def test_internal_cop_error_with_exit_one_is_rejected
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      assert_raises(RuntimeError) do
        runner.process([RbConfig.ruby, '-e', 'warn "An error occurred while a cop was inspecting"; exit 1'],
                       root, File.join(root, 'report.json'))
      end
    end
  end

  def test_source_text_that_mentions_an_internal_error_is_allowed
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      # 正常なソース行に同じ文言があっても、stderr の内部エラーと混同しない。
      result = runner.process([RbConfig.ruby, '-e', 'puts "An error occurred while"; exit 1'],
                              root, File.join(root, 'report.json'))
      assert_equal 1, result.fetch('exit')
    end
  end

  def test_timed_report_rejects_a_field_change_without_a_count_change
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      path = File.join(root, 'report.json')
      expected = SonicopBenchmark::Report.new(document)
      File.write(path, JSON.generate(document))
      assert_equal 1, runner.verify_run(path, expected).fetch('offenses')

      # 事前検査が正しくても、時間を測った実行で終端座標だけが変われば採用しない。
      changed = document
      changed.fetch('files').first.fetch('offenses').first.fetch('location')['last_column'] += 1
      File.write(path, JSON.generate(changed))
      assert_raises(RuntimeError) { runner.verify_run(path, expected) }
    end
  end

  def test_timed_report_rejects_a_truncated_inspection
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      changed = document
      changed.fetch('summary')['target_file_count'] = 2
      path = File.join(root, 'report.json')
      File.write(path, JSON.generate(changed))
      assert_raises(ArgumentError) { runner.verify_run(path, SonicopBenchmark::Report.new(document)) }
    end
  end

  def probe_command(version)
    report = document
    offense = report.fetch('files').first.fetch('offenses').first
    offense['cop_name'] = 'Lint/Syntax'
    offense['message'] = "Syntax error (Using Ruby #{version} parser)"
    [RbConfig.ruby, '-e', 'STDIN.read; puts ARGV.fetch(0); exit 1', JSON.generate(report), '--']
  end

  def test_parent_version_is_rejected_even_when_both_tools_agree
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      # 親 gemspec で両者とも2.6になった実測を再現する。同じ値であるだけでは足りない。
      commands = { 'reference' => probe_command('2.6'), 'candidate' => probe_command('2.6') }
      error = assert_raises(RuntimeError) { runner.target_versions('mastodon', root, commands) }
      assert_includes error.message, 'expected 4.0'
    end
  end

  def test_target_probe_requires_a_parseable_pinned_version
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      commands = { 'reference' => probe_command('4.0'), 'candidate' => probe_command('4.0') }
      assert_equal({ 'reference' => '4.0', 'candidate' => '4.0' }, runner.target_versions('mastodon', root, commands))
      commands['candidate'] = probe_command('unknown')
      assert_raises(RuntimeError) { runner.target_versions('mastodon', root, commands) }
    end
  end

  def test_timed_parallel_notice_is_required_exactly_once
    Dir.mktmpdir do |root|
      runner = SonicopBenchmark::Runner.new(candidate: RbConfig.ruby, reference: RbConfig.ruby,
                                           output: File.join(root, 'out'), repeats: 2)
      path = File.join(root, 'report.json')
      records = JSON.parse(File.read(File.expand_path('../tests/fixtures/parallel_option_notices.json', __dir__)))
      prefix = records.fetch(1).fetch('prefix')
      prefix = prefix.gsub("\n", "\r\n") if File::ALT_SEPARATOR
      text = JSON.generate(document)
      expected = SonicopBenchmark::Report.new(document)
      File.binwrite(path, prefix + text)
      assert_equal 1, runner.verify_run(path, expected, parallel_notice: true).fetch('offenses')
      assert_raises(JSON::ParserError) { runner.verify_run(path, expected) }
      File.binwrite(path, text)
      assert_raises(RuntimeError) { runner.verify_run(path, expected, parallel_notice: true) }
      File.binwrite(path, prefix + prefix + text)
      assert_raises(JSON::ParserError) { runner.verify_run(path, expected, parallel_notice: true) }
    end
  end
end
