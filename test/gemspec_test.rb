# frozen_string_literal: true

require 'test_helper'

# 「platform gem のつもりでソース gem が出る」「ソース gem に残骸バイナリが混入する」の
# 両方向を gemspec の段階で止められているかを、隔離したツリーの上で確かめる。
class GemspecTest < Minitest::Test
  include SonicopTestHelpers

  FIXTURE_FILES = %w[
    CONFORMANCE.md LICENSE NOTICE README.md README.ja.md
    Cargo.lock Cargo.toml build.rs
    config/default.yml licenses/NOTICE
    src/main.rs ext/sonicop/extconf.rb
  ].freeze

  def setup
    @root = Dir.mktmpdir('sonicop-gemspec')
    FIXTURE_FILES.each do |relative|
      path = File.join(@root, relative)
      FileUtils.mkdir_p(File.dirname(path))
      File.write(path, "fixture\n")
    end
    FileUtils.mkdir_p(File.join(@root, 'libexec'))
    FileUtils.cp(File.join(ROOT, 'sonicop.gemspec'), @root)
    FileUtils.mkdir_p(File.join(@root, 'lib', 'sonicop'))
    FileUtils.cp(File.join(ROOT, 'lib', 'sonicop', 'version.rb'), File.join(@root, 'lib', 'sonicop'))
    @platform = ENV['SONICOP_GEM_PLATFORM']
  end

  def teardown
    ENV['SONICOP_GEM_PLATFORM'] = @platform
    FileUtils.remove_entry(@root)
  end

  # 古い RubyGems は `x64-mingw-ucrt` のような新しい platform 文字列を解釈できず
  # `x64-unknown` に潰す。ここで見たいのは gemspec が指定をそのまま spec.platform へ
  # 渡すことなので、正規化そのものは期待値側に織り込む。
  def expected_platform(platform)
    Gem::Platform.new(platform).to_s
  end

  def evaluate_gemspec(platform: nil)
    ENV['SONICOP_GEM_PLATFORM'] = platform
    path = File.join(@root, 'sonicop.gemspec')
    verbose = $VERBOSE
    # コピーした version.rb を読み直すため、定数再定義の警告だけを黙らせる。
    $VERBOSE = nil
    eval(File.read(path), TOPLEVEL_BINDING, path)
  ensure
    $VERBOSE = verbose
  end

  def test_source_gem_ships_the_rust_sources_and_declares_the_extension
    spec = evaluate_gemspec

    assert_equal ['ext/sonicop/extconf.rb'], spec.extensions
    assert_includes spec.files, 'Cargo.toml'
    assert_includes spec.files, 'src/main.rs'
    assert_includes spec.files, 'lib/sonicop/version.rb'
    assert_includes spec.files, 'config/default.yml'
    refute(spec.files.any? { |path| path.start_with?('libexec/') })
  end

  # ソース gem は install 時に cargo build する。build.rs が欠けると build script が
  # 走らず、src/engine.rs の env!("SONICOP_BUILD_FINGERPRINT") が
  # `environment variable ... not defined at compile time` でコンパイルごと落ちる。
  # ルート直下のファイルは `src/**/*.rs` のような glob から漏れるので、明示して守る。
  def test_source_gem_ships_the_build_script
    spec = evaluate_gemspec

    assert_includes spec.files, 'build.rs'
  end

  # prebuilt 側は cargo を動かさないので、build script も死荷物にしかならない。
  def test_platform_gem_leaves_the_build_script_out
    stub_executable(@root, 'libexec', 'sonicop')

    spec = evaluate_gemspec(platform: 'x86_64-linux')

    refute_includes spec.files, 'build.rs'
  end

  def test_platform_gem_ships_the_binary_without_the_rust_sources
    stub_executable(@root, 'libexec', 'sonicop')

    spec = evaluate_gemspec(platform: 'x86_64-linux')

    assert_equal expected_platform('x86_64-linux'), spec.platform.to_s
    assert_empty spec.extensions
    assert_includes spec.files, 'libexec/sonicop'
    assert_includes spec.files, 'config/default.yml'
    refute_includes spec.files, 'Cargo.toml'
    refute_includes spec.files, 'Cargo.lock'
    refute_includes spec.files, 'src/main.rs'
    refute_includes spec.files, 'ext/sonicop/extconf.rb'
  end

  def test_windows_platform_gem_ships_the_exe
    stub_executable(@root, 'libexec', 'sonicop.exe')

    spec = evaluate_gemspec(platform: 'x64-mingw-ucrt')

    assert_equal expected_platform('x64-mingw-ucrt'), spec.platform.to_s
    assert_empty spec.extensions
    assert_includes spec.files, 'libexec/sonicop.exe'
  end

  # 中断した gem-platform が残した libexec/sonicop は .gitignore されるため
  # git status にも出ない。そのまま source gem を作らせない。
  def test_refuses_to_package_a_stale_binary_into_the_source_gem
    stub_executable(@root, 'libexec', 'sonicop')

    error = assert_raises(RuntimeError) { evaluate_gemspec }
    assert_includes error.message, 'libexec/sonicop'
    assert_includes error.message, 'make clean'
  end

  def test_refuses_to_build_a_platform_gem_without_a_binary
    error = assert_raises(RuntimeError) { evaluate_gemspec(platform: 'x86_64-linux') }
    assert_includes error.message, 'SONICOP_GEM_PLATFORM'
  end

  # gemspec の Dir.glob が cwd 相対だと、別ディレクトリから gem build したときに
  # platform 指定が効かずソース gem が出る。
  def test_file_list_does_not_depend_on_the_working_directory
    stub_executable(@root, 'libexec', 'sonicop')

    spec = Dir.chdir(Dir.tmpdir) { evaluate_gemspec(platform: 'arm64-darwin') }

    assert_equal expected_platform('arm64-darwin'), spec.platform.to_s
    assert_includes spec.files, 'libexec/sonicop'
  end
end
