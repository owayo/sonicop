# frozen_string_literal: true

require 'English'
require 'fileutils'
require 'rbconfig'
require 'tmpdir'
require 'minitest/autorun'

ROOT = File.expand_path('..', __dir__)

module SonicopTestHelpers
  # 実行ビットの有無で候補が選ばれる/飛ばされるかを見たいので、
  # スタブは中身ではなくパーミッションだけが意味を持つ。
  def stub_executable(*parts)
    path = File.join(*parts)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, "#!/bin/sh\nexit 0\n")
    FileUtils.chmod(0o755, path)
    path
  end

  def stub_unexecutable(*parts)
    path = File.join(*parts)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, '')
    FileUtils.chmod(0o644, path)
    path
  end
end
