# frozen_string_literal: true

require 'test_helper'
require 'open3'

class ExtconfTest < Minitest::Test
  def fixture
    Dir.mktmpdir('sonicop-extension') do |root|
      extension = File.join(root, 'ext/sonicop')
      FileUtils.mkdir_p(extension)
      FileUtils.cp(File.join(ROOT, 'ext/sonicop/extconf.rb'), extension)
      FileUtils.mkdir_p(File.join(root, 'src'))
      File.write(File.join(root, 'Cargo.toml'), <<~TOML)
        [package]
        name = "extension-fixture"
        version = "0.1.0"
        edition = "2021"
        [[bin]]
        name = "sonicop"
        path = "src/main.rs"
      TOML
      File.write(File.join(root, 'src/main.rs'), 'fn main() { println!("native fixture"); }')
      # ユーザーの共有 target-dir やクロスコンパイル指定へ小さな fixture を書き出さない。
      env = { 'CARGO_HOME' => File.join(root, '.cargo-home'), 'CARGO_TARGET_DIR' => nil,
              'CARGO_BUILD_TARGET' => nil, 'RUSTFLAGS' => nil, 'CARGO_ENCODED_RUSTFLAGS' => nil }
      yield root, extension, env
    end
  end

  def test_installs_the_binary_from_cargos_actual_target_directory
    skip '実ビルドと GNU Make の組み合わせは Unix の source gem 検査で扱う' if Gem.win_platform?

    %w[default absolute relative configuration].each do |mode|
      fixture do |root, extension, env|
        case mode
        when 'absolute'
          env['CARGO_TARGET_DIR'] = File.join(root, 'cache $literal')
        when 'relative'
          env['CARGO_TARGET_DIR'] = 'relative cache'
        when 'configuration'
          FileUtils.mkdir_p(File.join(root, '.cargo'))
          File.write(File.join(root, '.cargo/config.toml'), "[build]\ntarget-dir = 'configured cache'\n")
        end
        output, error, status = Open3.capture3(env, 'cargo', 'generate-lockfile', '--offline', chdir: root)
        assert status.success?, "#{mode}: #{output}\n#{error}"
        output, error, status = Open3.capture3(env, RbConfig.ruby, 'extconf.rb', chdir: extension)
        assert status.success?, "#{mode}: #{output}\n#{error}"
        destination = File.join(root, 'installed')
        output, error, status = Open3.capture3(env, 'make', 'install', "sitearchdir=#{destination}", chdir: extension)
        assert status.success?, "#{mode}: #{output}\n#{error}"
        # Makefile の文字列だけでなく、実際に導入したバイナリが動くことまで確認する。
        output, error, status = Open3.capture3(File.join(destination, 'sonicop-bin'))
        assert status.success?, "#{mode}: #{error}"
        assert_equal "native fixture\n", output
      end
    end
  end

  def test_stops_before_generating_a_makefile_when_cargo_metadata_fails
    fixture do |root, extension, env|
      File.write(File.join(root, 'Cargo.toml'), 'invalid manifest')
      output, error, status = Open3.capture3(env, RbConfig.ruby, 'extconf.rb', chdir: extension)
      refute status.success?, output
      assert_includes error, 'cargo metadata failed:'
      refute File.exist?(File.join(extension, 'Makefile'))
    end
  end

  def test_installs_with_backslashes_in_the_windows_cargo_path
    skip 'Windows の PATH 表記を使う実ビルド' unless Gem.win_platform?

    git_bin = File.join(ENV.fetch('ProgramFiles', 'C:/Program Files'), 'Git/usr/bin')
    skip 'Makefile の POSIX レシピに Git Bash が必要' unless File.file?(File.join(git_bin, 'sh.exe'))

    fixture do |root, extension, env|
      # native の find_executable が返す区切りを、Make と sh の両方へ通す。
      env['PATH'] = ([git_bin] + ENV.fetch('PATH').split(File::PATH_SEPARATOR))
                    .map { |path| path.tr('/', '\\') }.join(File::PATH_SEPARATOR)
      output, error, status = Open3.capture3(env, 'cargo', 'generate-lockfile', '--offline', chdir: root)
      assert status.success?, "#{output}\n#{error}"
      output, error, status = Open3.capture3(env, RbConfig.ruby, 'extconf.rb', chdir: extension)
      assert status.success?, "#{output}\n#{error}"
      destination = File.join(root, 'installed')
      output, error, status = Open3.capture3(env, 'make', 'install', "sitearchdir=#{destination}", chdir: extension)
      assert status.success?, "#{output}\n#{error}"
      output, error, status = Open3.capture3(File.join(destination, 'sonicop-bin.exe'))
      assert status.success?, error
      assert_equal "native fixture\n", output
    end
  end
end
