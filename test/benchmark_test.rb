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
end
