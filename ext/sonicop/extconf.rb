# frozen_string_literal: true

require 'mkmf'
require 'rbconfig'
require 'shellwords'

cargo = find_executable('cargo')
abort 'cargo is required to build the sonicop source gem' unless cargo

root = File.expand_path('../..', __dir__)
executable = Gem.win_platform? ? 'sonicop.exe' : 'sonicop'
installed_executable = Gem.win_platform? ? 'sonicop-bin.exe' : 'sonicop-bin'
binary = File.join(root, 'target', 'release', executable)

create_makefile('sonicop/native')

File.open('Makefile', 'a') do |makefile|
  makefile.write(<<~MAKEFILE)

    SONICOP_ROOT = #{Shellwords.escape(root)}
    SONICOP_CARGO = #{Shellwords.escape(cargo)}
    SONICOP_BINARY = #{Shellwords.escape(binary)}
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
