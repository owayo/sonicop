# frozen_string_literal: true

require 'json'
require 'open3'
require 'test_helper'

class ConformanceDiffTest < Minitest::Test
  SCRIPT = File.join(ROOT, 'scripts', 'conformance_diff.sh')

  def setup
    # Windows は shebang を見ないので、`.sh` を直接 exec すると jq の有無に関わらず
    # `Errno::ENOEXEC: Exec format error` になる。conformance_diff.sh は適合率を測る
    # ための保守用ツールで、製品が通る経路ではない。bash を探して回るより、
    # jq が無いときと同じように名指しで飛ばす。
    skip 'conformance_diff.sh is a bash script and cannot be exec\'d on Windows' if Gem.win_platform?
    skip 'conformance_diff.sh requires jq' unless system('jq', '--version', out: File::NULL, err: File::NULL)

    @root = Dir.mktmpdir('sonicop-conformance-diff')
    @reference = fake_linter('reference')
    @candidate = fake_linter('candidate')
  end

  def teardown
    FileUtils.remove_entry(@root) if @root
  end

  def test_separates_syntax_file_acceptance_from_positions_inside_shared_error_files
    write_result(
      @reference,
      'shared.rb' => [syntax(1, '$shared'), syntax(3, 'kEND')],
      'reference-only.rb' => [syntax(1, '$end')],
      'candidate-only.rb' => [],
      'clean.rb' => [offense('Layout/Common', 1), offense('Lint/ReferenceOnly', 2)]
    )
    write_result(
      @candidate,
      'shared.rb' => [syntax(1, '$shared'), syntax(2, 'tCOMMA')],
      'reference-only.rb' => [],
      'candidate-only.rb' => [syntax(1, '$end')],
      'clean.rb' => [offense('Layout/Common', 1), offense('Lint/CandidateOnly', 3)]
    )

    output, status, artifacts = compare

    assert_equal 1, status.exitstatus
    assert_includes output, 'syntax-error files : reference=2 candidate=2 shared=1'
    assert_includes output, 'syntax file diff   : candidate-only=1 reference-only=1'
    assert_includes output, 'candidate-only pos : 3  (actionable=2 / shared syntax files=1)'
    assert_includes output, 'reference-only pos : 3  (actionable=2 / shared syntax files=1)'
    assert_includes output, 'candidate recovery : after-shared=1 / without-earlier-shared=0'
    assert_equal "shared.rb\tLint/Syntax\t2\t1\n",
                 File.read(File.join(artifacts, 'syntax_recovery_candidate_after_shared.tsv'))
  end

  def test_excluding_shared_syntax_error_files_can_prove_the_remaining_offenses_match
    write_result(
      @reference,
      'shared.rb' => [syntax(1, '$shared'), syntax(3, 'kEND')],
      'clean.rb' => [offense('Layout/Common', 1)]
    )
    write_result(
      @candidate,
      'shared.rb' => [syntax(1, '$shared'), syntax(2, 'tCOMMA')],
      'clean.rb' => [offense('Layout/Common', 1)]
    )

    output, status, = compare('--exclude-unparsable')

    assert_predicate status, :success?
    assert_includes output, 'syntax-error files : reference=1 candidate=1 shared=1'
    assert_includes output, 'candidate-only pos : 0  (actionable=0 / shared syntax files=0)'
    assert_includes output, 'reference-only pos : 0  (actionable=0 / shared syntax files=0)'
    assert_includes output, '完全一致'
  end

  private

  def fake_linter(name)
    command = File.join(@root, name)
    json = "#{command}.json"
    File.write(command, "#!#{RbConfig.ruby}\nprint File.binread(#{json.dump})\nexit 1\n")
    FileUtils.chmod(0o755, command)
    command
  end

  def write_result(command, files)
    document = {
      'files' => files.map { |path, offenses| { 'path' => path, 'offenses' => offenses } }
    }
    File.write("#{command}.json", JSON.generate(document))
  end

  def syntax(line, token)
    offense('Lint/Syntax', line, severity: 'fatal', message: "unexpected token #{token}")
  end

  def offense(cop, line, severity: 'warning', message: cop)
    {
      'cop_name' => cop,
      'severity' => severity,
      'correctable' => false,
      'message' => message,
      'location' => { 'line' => line, 'column' => 1, 'length' => 1 }
    }
  end

  def compare(*options)
    artifacts = File.join(@root, "artifacts-#{Dir.children(@root).count}")
    stdout, stderr, status = Open3.capture3(
      SCRIPT,
      '-r', @reference,
      '-c', @candidate,
      '-o', artifacts,
      '--quiet',
      *options,
      '--', '.'
    )
    [stdout + stderr, status, artifacts]
  end
end
