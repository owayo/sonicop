# frozen_string_literal: true

require 'json'
require 'open3'
require 'test_helper'

class ConformanceTableTest < Minitest::Test
  SCRIPT = File.join(ROOT, 'scripts', 'conformance_table.rb')
  COP = 'Layout/Example'

  def setup
    @root = Dir.mktmpdir('sonicop-conformance-table')
    @cops = File.join(@root, 'cops.txt')
    File.write(@cops, "#{COP}\n")
    @runs = []
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def test_matching_complete_reports_pass
    add_run(report, report)

    output, status = compare

    assert_predicate status, :success?
    assert_includes output, '| **Total** | **1** | **1** | **1** | **0** |'
  end

  def test_length_difference_is_diverging_even_when_end_coordinates_match
    add_run(report(length: 1), report(length: 2))

    output, status = compare

    assert_equal 1, status.exitstatus
    assert_includes output, '| **Total** | **1** | **1** | **0** | **1** |'
  end

  def test_offense_multiplicity_is_preserved
    reference = report
    reference['files'].first['offenses'] *= 2
    add_run(reference, report)

    output, status = compare

    assert_equal 1, status.exitstatus
    assert_includes output, '| **Total** | **1** | **1** | **0** | **1** |'
  end

  def test_opposite_differences_in_separate_runs_do_not_cancel
    # 別コーパスの同名ファイルを混ぜると、不足と過剰が互いに打ち消し合う。
    add_run(report, report(offenses: []))
    add_run(report(offenses: []), report)

    output, status = compare

    assert_equal 1, status.exitstatus
    assert_includes output, '| **Total** | **1** | **1** | **0** | **1** |'
  end

  def test_silent_cops_do_not_count_as_exact
    add_run(report(offenses: []), report(offenses: []))

    output, status = compare

    assert_equal 1, status.exitstatus
    assert_includes output, '| **Total** | **1** | **0** | **0** | **0** |'
  end

  def test_incomplete_reference_is_rejected_before_printing_table
    reference = report
    reference['summary']['target_file_count'] = 2
    add_run(reference, report)

    assert_invalid_report('incomplete inspection')
  end

  def test_zero_inspected_files_are_rejected
    document = { 'files' => [], 'summary' => { 'target_file_count' => 0, 'inspected_file_count' => 0 } }
    add_run(document, document)

    assert_invalid_report('incomplete inspection')
  end

  def test_different_file_sets_are_rejected_even_when_counts_match
    candidate = report
    candidate['files'].first['path'] = 'other.rb'
    add_run(report, candidate)

    assert_invalid_report('different target paths')
  end

  def test_missing_summary_is_rejected
    candidate = report
    candidate.delete('summary')
    add_run(report, candidate)

    assert_invalid_report('summary')
  end

  def test_null_required_offense_values_are_rejected
    %w[line column last_line last_column length message severity correctable].each do |field|
      document = report
      offense = document['files'].first['offenses'].first
      owner = offense['location'].key?(field) ? offense['location'] : offense
      owner[field] = nil
      @runs.clear
      add_run(document, document)

      assert_invalid_report('invalid offense fields')
    end
  end

  def test_null_files_are_reported_as_unmeasured
    document = report
    document['files'] = nil
    add_run(document, document)

    assert_invalid_report('invalid files')
  end

  def test_unlisted_cops_are_rejected
    candidate = report
    candidate['files'].first['offenses'].first['cop_name'] = 'Lint/Unknown'
    add_run(report, candidate)

    assert_invalid_report('unlisted cop')
  end

  def test_duplicate_cop_names_are_rejected
    File.write(@cops, "#{COP}\n#{COP}\n")
    add_run(report, report)

    assert_invalid_report('duplicate cop')
  end

  def test_empty_run_list_is_rejected
    assert_invalid_report('no runs')
  end

  def test_missing_argument_is_reported_as_unmeasured
    stdout, stderr, status = Open3.capture3(RbConfig.ruby, SCRIPT)

    assert_equal 2, status.exitstatus
    assert_includes stdout + stderr, 'missing spec path'
  end

  def test_non_mapping_spec_is_reported_as_unmeasured
    path = File.join(@root, 'invalid-spec.json')
    File.write(path, '[]')
    stdout, stderr, status = Open3.capture3(RbConfig.ruby, SCRIPT, path)

    assert_equal 2, status.exitstatus
    assert_includes stdout + stderr, 'invalid spec'
  end

  def test_null_report_path_is_reported_as_unmeasured
    add_run(report, report)
    @runs.first['reference'] = nil

    assert_invalid_report('invalid report path')
  end

  private

  def report(length: 1, offenses: nil)
    {
      'files' => [{
        'path' => 'example.rb',
        'offenses' => offenses || [{
          'cop_name' => COP, 'message' => 'Example.', 'severity' => 'convention', 'correctable' => true,
          'location' => { 'line' => 1, 'column' => 1, 'last_line' => 1, 'last_column' => 1, 'length' => length }
        }]
      }],
      'summary' => { 'target_file_count' => 1, 'inspected_file_count' => 1 }
    }
  end

  def add_run(reference, candidate)
    number = @runs.length
    paths = %w[reference candidate].map { |side| File.join(@root, "#{side}-#{number}.json") }
    [reference, candidate].zip(paths).each { |document, path| File.write(path, JSON.generate(document)) }
    @runs << { 'label' => "run-#{number}", 'root' => @root, 'reference' => paths[0], 'candidate' => paths[1] }
  end

  def compare
    spec = File.join(@root, 'spec.json')
    File.write(spec, JSON.generate('cops' => @cops, 'runs' => @runs))
    stdout, stderr, status = Open3.capture3(RbConfig.ruby, SCRIPT, spec)
    [stdout + stderr, status]
  end

  def assert_invalid_report(message)
    output, status = compare
    assert_equal 2, status.exitstatus
    assert_includes output, message
    refute_includes output, '| Department |'
  end
end
