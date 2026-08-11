# frozen_string_literal: true

require 'test_helper'
require 'sonicop'

# gem 配布で一番壊れやすいのがこの候補解決なので、順序と fallback を固定する。
class RunnerTest < Minitest::Test
  include SonicopTestHelpers

  FakeSpecification = Struct.new(:extension_dir, :full_gem_path)

  # 候補名は実行環境に合わせる。Windows の `File.executable?` は拡張子で可否を決めるため、
  # 拡張子の無いファイルは「存在するが実行不可」になる。Unix 側の命名のまま Windows で
  # 走らせても、製品が実際に通る経路 (`sonicop.exe` を探す) を検証したことにならない。
  WINDOWS = Gem.win_platform?
  EXECUTABLE = WINDOWS ? 'sonicop.exe' : 'sonicop'
  INSTALLED_EXECUTABLE = WINDOWS ? 'sonicop-bin.exe' : 'sonicop-bin'

  def setup
    @root = Dir.mktmpdir('sonicop-runner')
    @specification = FakeSpecification.new(File.join(@root, 'extensions'), File.join(@root, 'gem'))
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def find(env: {}, specification: @specification, windows: WINDOWS)
    Sonicop::Runner.find_binary(root: @root, env: env, specification: specification, windows: windows)
  end

  def candidates(specification: @specification, windows: WINDOWS)
    Sonicop::Runner.candidates(root: @root, env: {}, specification: specification, windows: windows)
  end

  def test_candidate_order_is_the_documented_contract
    override = File.join(@root, 'override', EXECUTABLE)
    expected = [
      override,
      File.join(@root, 'libexec', EXECUTABLE),
      File.join(@specification.extension_dir, EXECUTABLE),
      File.join(@specification.extension_dir, INSTALLED_EXECUTABLE),
      File.join(@specification.full_gem_path, 'lib', INSTALLED_EXECUTABLE),
      File.join(@root, 'target', 'release', EXECUTABLE),
      File.join(@root, 'target', 'debug', EXECUTABLE)
    ]

    actual = Sonicop::Runner.candidates(
      root: @root, env: { 'SONICOP_BINARY' => override }, specification: @specification, windows: WINDOWS
    )
    assert_equal expected, actual
  end

  def test_explicit_override_wins_over_every_other_candidate
    override = stub_executable(@root, 'override', EXECUTABLE)
    stub_executable(@root, 'libexec', EXECUTABLE)
    stub_executable(@specification.extension_dir, EXECUTABLE)

    assert_equal override, find(env: { 'SONICOP_BINARY' => override })
  end

  def test_libexec_wins_over_extension_dir_and_development_tree
    libexec = stub_executable(@root, 'libexec', EXECUTABLE)
    stub_executable(@specification.extension_dir, EXECUTABLE)
    stub_executable(@root, 'target', 'release', EXECUTABLE)

    assert_equal libexec, find
  end

  def test_extension_dir_wins_over_development_tree
    extension = stub_executable(@specification.extension_dir, EXECUTABLE)
    stub_executable(@root, 'target', 'release', EXECUTABLE)

    assert_equal extension, find
  end

  def test_falls_back_to_the_installed_native_executable_name
    native = stub_executable(@specification.extension_dir, INSTALLED_EXECUTABLE)
    stub_executable(@root, 'target', 'debug', EXECUTABLE)

    assert_equal native, find
  end

  def test_falls_back_to_the_native_executable_inside_the_gem_lib_directory
    native = stub_executable(@specification.full_gem_path, 'lib', INSTALLED_EXECUTABLE)

    assert_equal native, find
  end

  def test_release_build_wins_over_debug_build
    release = stub_executable(@root, 'target', 'release', EXECUTABLE)
    stub_executable(@root, 'target', 'debug', EXECUTABLE)

    assert_equal release, find
  end

  def test_skips_candidates_without_the_executable_bit
    skip 'POSIX permission bits only' if Gem.win_platform?

    stub_unexecutable(@root, 'libexec', EXECUTABLE)
    release = stub_executable(@root, 'target', 'release', EXECUTABLE)

    assert_equal release, find
  end

  def test_skips_an_override_that_is_not_executable
    skip 'POSIX permission bits only' if Gem.win_platform?

    override = stub_unexecutable(@root, 'override', EXECUTABLE)
    libexec = stub_executable(@root, 'libexec', EXECUTABLE)

    assert_equal libexec, find(env: { 'SONICOP_BINARY' => override })
  end

  def test_skips_directories_that_share_the_executable_name
    FileUtils.mkdir_p(File.join(@root, 'libexec', EXECUTABLE))
    release = stub_executable(@root, 'target', 'release', EXECUTABLE)

    assert_equal release, find
  end

  def test_returns_nil_when_no_candidate_exists
    assert_nil find
  end

  def test_source_checkout_without_an_installed_gem_skips_gem_candidates
    release = stub_executable(@root, 'target', 'release', EXECUTABLE)

    assert_equal release, find(specification: nil)
    assert_equal 3, candidates(specification: nil).size
  end

  def test_windows_looks_for_exe_names
    assert_equal(
      [
        File.join(@root, 'libexec', 'sonicop.exe'),
        File.join(@specification.extension_dir, 'sonicop.exe'),
        File.join(@specification.extension_dir, 'sonicop-bin.exe'),
        File.join(@specification.full_gem_path, 'lib', 'sonicop-bin.exe'),
        File.join(@root, 'target', 'release', 'sonicop.exe'),
        File.join(@root, 'target', 'debug', 'sonicop.exe')
      ],
      candidates(windows: true)
    )
  end

  def test_default_root_is_the_gem_root
    assert_equal ROOT, Sonicop::Runner.gem_root
  end

  # ABI が合わない platform gem が入ると exec 自体が errno で落ちる。素の errno だけでは
  # 利用者が原因にたどり着けないので、復旧手段まで案内できているかを見る。
  def test_reports_an_unusable_binary_instead_of_a_bare_errno
    skip 'relies on POSIX interpreter lookup' if Gem.win_platform?

    unusable = File.join(@root, 'libexec', EXECUTABLE)
    FileUtils.mkdir_p(File.dirname(unusable))
    File.write(unusable, "#!/sonicop/no/such/interpreter\n")
    FileUtils.chmod(0o755, unusable)

    output = IO.popen(
      [{ 'SONICOP_BINARY' => unusable }, RbConfig.ruby, '-I', File.join(ROOT, 'lib'),
       '-rsonicop', '-e', 'Sonicop::Runner.run([])'],
      err: [:child, :out], &:read
    )

    refute_predicate $CHILD_STATUS, :success?
    assert_includes output, 'could not be started'
    assert_includes output, 'gem install sonicop --platform ruby'
  end
end
