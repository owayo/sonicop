# frozen_string_literal: true

require 'fileutils'

# Cargo.toml を正本とするバージョン管理のヘルパー。
#
# バイナリの `--version` は CARGO_PKG_VERSION 由来なので、Cargo.toml を上げ忘れると
# gem の版と中身が名乗る版が割れる。Ruby 側からは TOML を読めないため、
# `[package]` セクションの version だけを読む最小のパーサをここに置き、
# lib/sonicop/version.rb はここから生成する (生成物は gem に同梱するのでコミットする)。
module SonicopVersion
  ROOT = File.expand_path('..', __dir__)
  CARGO_TOML = 'Cargo.toml'
  CARGO_LOCK = 'Cargo.lock'
  VERSION_RB = File.join('lib', 'sonicop', 'version.rb')

  Mismatch = Class.new(StandardError)

  # `[section]` と `[[section]]` の両方を拾う。`[[bin]]` を見落とすと
  # その配下のキーが `[package]` の続きに見えてしまう。
  SECTION = /\A\[{1,2}([^\[\]]+)\]{1,2}\z/.freeze
  CARGO_VERSION = /\Aversion\s*=\s*"([^"]+)"/.freeze
  CARGO_NAME = /\Aname\s*=\s*"([^"]+)"/.freeze
  RUBY_VERSION_LITERAL = /^\s*VERSION\s*=\s*['"]([^'"]+)['"]/.freeze

  # Cargo は SemVer を厳格に検証し、数値識別子の先頭ゼロを拒否する
  # (`26.08.100` は `invalid leading zero in minor version number`)。
  # RubyGems 側は素通しするうえ `26.08.100` と `26.8.100` を同一版と見なすため、
  # ゼロ埋めを一度でも公開すると綴りの揺れが再 push 不能の衝突になる。
  # 採番の入口で弾き、CI の終盤ではなく手元で気づけるようにする。
  SEMVER_CORE = /\A(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:[-+][0-9A-Za-z.-]+)?\z/.freeze

  module_function

  def cargo_version(root = ROOT)
    section = nil
    File.foreach(File.join(root, CARGO_TOML)) do |line|
      stripped = line.strip
      next if stripped.empty? || stripped.start_with?('#')

      if (header = stripped[SECTION, 1])
        section = header.strip
        next
      end
      next unless section == 'package'

      captured = stripped[CARGO_VERSION, 1]
      return captured if captured
    end
    raise Mismatch, "#{CARGO_TOML} の [package] に version が見つかりません"
  end

  def gem_version(root = ROOT)
    path = File.join(root, VERSION_RB)
    captured = File.read(path)[RUBY_VERSION_LITERAL, 1]
    raise Mismatch, "#{VERSION_RB} に VERSION の定義が見つかりません" unless captured

    captured
  end

  def render(version)
    <<~RUBY
      # frozen_string_literal: true

      # Generated from Cargo.toml by `rake version:sync`. Do not edit by hand.
      module Sonicop
        VERSION = '#{version}'
      end
    RUBY
  end

  # Returns true when version.rb was rewritten.
  def sync!(root = ROOT)
    version = cargo_version(root)
    path = File.join(root, VERSION_RB)
    rendered = render(version)
    return false if File.exist?(path) && File.read(path) == rendered

    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, rendered)
    true
  end

  # Cargo.toml / Cargo.lock / version.rb の 3 つを新しい版へ揃える。
  #
  # 行頭一致の sed で済ませないのは、`0,/re/` が GNU 拡張で BSD sed では**無言で何もしない**
  # ため。書き換え失敗に気づけるのが CI 終盤の照合になり、しかもリリースは不可逆に近い。
  # 読み取りと同じ section 追跡で書き、対象が見つからなければその場で失敗させる。
  def set!(version, root = ROOT)
    version = version.to_s.strip
    raise Mismatch, 'バージョンが空です' if version.empty?
    unless SEMVER_CORE.match?(version)
      raise Mismatch, "Cargo が受け付けない版です (先頭ゼロや桁不足): #{version}"
    end

    write_cargo_version(version, root)
    write_lock_version(version, root)
    sync!(root)
    version
  end

  def cargo_name(root = ROOT)
    section = nil
    File.foreach(File.join(root, CARGO_TOML)) do |line|
      stripped = line.strip
      next if stripped.empty? || stripped.start_with?('#')

      if (header = stripped[SECTION, 1])
        section = header.strip
        next
      end
      next unless section == 'package'

      captured = stripped[CARGO_NAME, 1]
      return captured if captured
    end
    raise Mismatch, "#{CARGO_TOML} の [package] に name が見つかりません"
  end

  def write_cargo_version(version, root = ROOT)
    path = File.join(root, CARGO_TOML)
    section = nil
    replaced = false
    lines = File.readlines(path).map do |line|
      stripped = line.strip
      if !stripped.empty? && !stripped.start_with?('#') && (header = stripped[SECTION, 1])
        section = header.strip
        next line
      end
      next line unless section == 'package' && !replaced && stripped.match?(CARGO_VERSION)

      replaced = true
      line.sub(/"[^"]*"/, %("#{version}"))
    end
    raise Mismatch, "#{CARGO_TOML} の [package] に version が見つかりません" unless replaced

    File.write(path, lines.join)
  end

  # lock 全体を再解決すると無関係な依存まで動く。自パッケージの entry だけ追従させる。
  def write_lock_version(version, root = ROOT)
    path = File.join(root, CARGO_LOCK)
    return false unless File.exist?(path)

    name = cargo_name(root)
    lines = File.readlines(path)
    index = lines.index { |line| line.strip == %(name = "#{name}") }
    raise Mismatch, "#{CARGO_LOCK} に #{name} の entry が見つかりません" if index.nil?

    target = lines[index + 1]
    raise Mismatch, "#{CARGO_LOCK} の #{name} に version 行が続きません" unless target&.strip&.match?(CARGO_VERSION)

    lines[index + 1] = target.sub(/"[^"]*"/, %("#{version}"))
    File.write(path, lines.join)
    true
  end

  def check!(root = ROOT)
    cargo = cargo_version(root)
    gem = gem_version(root)
    return cargo if cargo == gem

    raise Mismatch, <<~MESSAGE
      バージョンが一致しません: #{CARGO_TOML}=#{cargo} #{VERSION_RB}=#{gem}
      Cargo.toml が正本です。`rake version:sync` を実行して差分をコミットしてください。
    MESSAGE
  end
end
