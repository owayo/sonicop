# frozen_string_literal: true

require 'test_helper'
require File.join(ROOT, 'script', 'sonicop_version')

# Cargo.toml が正本で lib/sonicop/version.rb は生成物、という取り決めを固定する。
class VersionTest < Minitest::Test
  include SonicopTestHelpers

  def setup
    @root = Dir.mktmpdir('sonicop-version')
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def write_cargo_toml(body)
    File.write(File.join(@root, 'Cargo.toml'), body)
  end

  def write_version_rb(version)
    path = File.join(@root, 'lib', 'sonicop', 'version.rb')
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, SonicopVersion.render(version))
    path
  end

  def test_repository_is_consistent
    assert_equal SonicopVersion.cargo_version, SonicopVersion.check!
  end

  def test_committed_version_rb_matches_the_generator_output
    path = File.join(ROOT, 'lib', 'sonicop', 'version.rb')

    assert_equal SonicopVersion.render(SonicopVersion.cargo_version), File.read(path),
                 'lib/sonicop/version.rb is stale; run `rake version:sync`'
  end

  def test_reads_the_version_from_the_package_section
    write_cargo_toml(<<~TOML)
      [package]
      name = "sonicop"
      version = "1.2.3"
    TOML

    assert_equal '1.2.3', SonicopVersion.cargo_version(@root)
  end

  # `[[bin]]` をセクション見出しとして認識できないと、その配下のキーが
  # [package] の続きに見えて別の version を拾ってしまう。
  def test_is_not_confused_by_double_bracket_sections_or_dependency_versions
    write_cargo_toml(<<~TOML)
      # version = "0.0.0"
      [package]
      name = "sonicop"
      version = "4.5.6"
      edition = "2024"

      [[bin]]
      name = "sonicop"
      path = "src/main.rs"

      [dependencies]
      version = "9.9.9"
    TOML

    assert_equal '4.5.6', SonicopVersion.cargo_version(@root)
  end

  def test_ignores_a_version_key_that_precedes_the_package_section
    write_cargo_toml(<<~TOML)
      [workspace.package]
      version = "0.0.1"

      [package]
      version = "7.8.9"
    TOML

    assert_equal '7.8.9', SonicopVersion.cargo_version(@root)
  end

  def test_raises_when_the_package_version_is_absent
    write_cargo_toml("[package]\nname = \"sonicop\"\n")

    assert_raises(SonicopVersion::Mismatch) { SonicopVersion.cargo_version(@root) }
  end

  def test_check_raises_when_the_two_files_disagree
    write_cargo_toml("[package]\nversion = \"2.0.0\"\n")
    write_version_rb('1.0.0')

    error = assert_raises(SonicopVersion::Mismatch) { SonicopVersion.check!(@root) }
    assert_includes error.message, '2.0.0'
    assert_includes error.message, '1.0.0'
  end

  def test_sync_writes_version_rb_and_is_idempotent
    write_cargo_toml("[package]\nversion = \"3.1.4\"\n")
    write_version_rb('0.0.0')

    assert SonicopVersion.sync!(@root)
    assert_equal '3.1.4', SonicopVersion.gem_version(@root)
    refute SonicopVersion.sync!(@root)
    assert_equal '3.1.4', SonicopVersion.check!(@root)
  end

  def write_cargo_lock(body)
    File.write(File.join(@root, 'Cargo.lock'), body)
  end

  # 採番は 3 ファイルを揃えて初めて意味を持つ。どれか 1 つでも取り残すと、
  # タグと中身が食い違った gem を publish しかけることになる。
  def test_set_updates_cargo_toml_lock_and_version_rb_together
    write_cargo_toml(<<~TOML)
      [package]
      name = "sonicop"
      version = "0.1.0"

      [dependencies]
      clap = { version = "4.6.4" }
    TOML
    write_cargo_lock(<<~LOCK)
      [[package]]
      name = "clap"
      version = "4.6.4"

      [[package]]
      name = "sonicop"
      version = "0.1.0"
      dependencies = [
       "clap",
      ]
    LOCK

    SonicopVersion.set!('26.8.100', @root)

    assert_equal '26.8.100', SonicopVersion.cargo_version(@root)
    assert_equal '26.8.100', SonicopVersion.gem_version(@root)
    lock = File.read(File.join(@root, 'Cargo.lock'))
    assert_includes lock, %(name = "sonicop"\nversion = "26.8.100")
    # 依存の版を巻き添えにすると、ビルドが通らないか別の版を掴む。
    assert_includes lock, %(name = "clap"\nversion = "4.6.4")
    assert_includes File.read(File.join(@root, 'Cargo.toml')), 'clap = { version = "4.6.4" }'
  end

  # Cargo は先頭ゼロを拒否し、RubyGems は受け入れたうえで 26.08.100 と 26.8.100 を
  # 同一版と見なす。公開後に気づいても、その版は二度と出せない。
  def test_set_rejects_versions_cargo_would_refuse
    write_cargo_toml("[package]\nname = \"sonicop\"\nversion = \"0.1.0\"\n")

    ['26.08.100', '26.8.001', '26.8', '', 'v26.8.100'].each do |rejected|
      assert_raises(SonicopVersion::Mismatch, "accepted #{rejected.inspect}") do
        SonicopVersion.set!(rejected, @root)
      end
    end
    assert_equal '0.1.0', SonicopVersion.cargo_version(@root)
  end

  def test_set_accepts_prerelease_and_build_metadata
    write_cargo_toml("[package]\nname = \"sonicop\"\nversion = \"0.1.0\"\n")

    SonicopVersion.set!('26.8.100-rc.1', @root)

    assert_equal '26.8.100-rc.1', SonicopVersion.cargo_version(@root)
  end

  # Cargo.lock を持たないチェックアウトでも採番は成立させる。
  def test_set_without_a_lock_file
    write_cargo_toml("[package]\nname = \"sonicop\"\nversion = \"0.1.0\"\n")

    SonicopVersion.set!('26.8.100', @root)

    assert_equal '26.8.100', SonicopVersion.cargo_version(@root)
  end

  def test_generated_version_rb_is_loadable_ruby
    write_cargo_toml("[package]\nversion = \"5.6.7\"\n")
    SonicopVersion.sync!(@root)
    path = File.join(@root, 'lib', 'sonicop', 'version.rb')

    loaded = IO.popen([RbConfig.ruby, '-r', path, '-e', 'print Sonicop::VERSION'], &:read)

    assert_equal '5.6.7', loaded
  end
end
