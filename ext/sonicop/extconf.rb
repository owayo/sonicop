# frozen_string_literal: true

require 'mkmf'
require 'json'
require 'open3'
require 'rbconfig'
require 'shellwords'

cargo = find_executable('cargo')
abort 'cargo is required to build the sonicop source gem' unless cargo

root = File.expand_path('../..', __dir__)
executable = Gem.win_platform? ? 'sonicop.exe' : 'sonicop'
installed_executable = Gem.win_platform? ? 'sonicop-bin.exe' : 'sonicop-bin'
# 環境変数や Cargo 設定の target-dir は build でも有効なので、出力場所を決め打ちしない。
metadata, errors, status = Open3.capture3(cargo, 'metadata', '--no-deps', '--format-version', '1', '--locked', chdir: root)
abort "cargo metadata failed: #{errors.strip}" unless status.success?

target_directory = JSON.parse(metadata).fetch('target_directory')
binary = File.join(File.expand_path(target_directory, root), 'release', executable)
# Make は shell より先に $ を処理するため、パスに含まれる文字として保持する。
quote = ->(value) { Shellwords.escape(value).gsub('$', '$$') }

create_makefile('sonicop/native')

File.open('Makefile', 'a') do |makefile|
  makefile.write(<<~MAKEFILE)

    SONICOP_ROOT = #{quote.call(root)}
    SONICOP_CARGO = #{quote.call(cargo)}
    SONICOP_BINARY = #{quote.call(binary)}
    SONICOP_EXE = #{installed_executable}

    .PHONY: sonicop-build sonicop-install

    all: sonicop-build

    sonicop-build:
		cd $(SONICOP_ROOT) && $(SONICOP_CARGO) build --release --locked

    install-so: sonicop-install

    sonicop-install: sonicop-build
		$(MAKEDIRS) $(sitearchdir)
		$(INSTALL_PROG) $(SONICOP_BINARY) $(sitearchdir)/$(SONICOP_EXE)
  MAKEFILE
end
