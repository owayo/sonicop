# frozen_string_literal: true

require_relative 'lib/sonicop/version'

Gem::Specification.new do |spec|
  spec.name = 'sonicop'
  spec.version = Sonicop::VERSION
  spec.authors = ['Yohei']
  spec.email = []

  spec.summary = 'A fast, native RuboCop-compatible Ruby linter and formatter'
  spec.description = <<~DESCRIPTION
    Sonicop is a Rust implementation of the core RuboCop inspection pipeline.
    It ships as a native executable wrapped in a Ruby gem.
  DESCRIPTION
  spec.homepage = 'https://github.com/owayo/sonicop'
  spec.license = 'MIT'
  spec.required_ruby_version = '>= 2.6.0'

  spec.metadata = {
    'homepage_uri' => spec.homepage,
    'source_code_uri' => spec.homepage,
    'changelog_uri' => "#{spec.homepage}/releases",
    'rubygems_mfa_required' => 'true'
  }

  root = __dir__
  prebuilt_platform = ENV['SONICOP_GEM_PLATFORM']
  prebuilt_binary = Dir.glob(['libexec/sonicop', 'libexec/sonicop.exe'], base: root)
                       .find { |path| File.file?(File.join(root, path)) }

  # platform gem とソース gem の取り違えは「静かに壊れた gem を配る」経路になるので、
  # 中途半端な状態では gem を作らせない。
  # - platform 指定があるのにバイナリが無い -> ソース gem が platform gem の名前で出る
  # - バイナリが残っているのに platform 未指定 -> ソース gem に他 OS のバイナリが混入し、
  #   Runner#find_binary が libexec を優先するため Exec format error になる
  if prebuilt_platform && prebuilt_binary.nil?
    raise 'SONICOP_GEM_PLATFORM is set but no prebuilt binary exists under libexec/. ' \
          'Build the platform gem with `make gem-platform GEM_PLATFORM=...`.'
  end
  if prebuilt_binary && prebuilt_platform.nil?
    raise "Stale #{prebuilt_binary} would be packaged into the source gem. " \
          'Remove it with `make clean` before building the source gem.'
  end

  shared_files = %w[
    CONFORMANCE.md
    LICENSE
    NOTICE
    README.md
    README.ja.md
    config/**/*
    licenses/**/*
    lib/**/*.rb
  ]
  # ソース gem だけが Cargo でビルドする。prebuilt 側に Rust ソースを積んでも
  # 絶対にビルドされない死荷物にしかならない。
  source_build_files = %w[Cargo.lock Cargo.toml ext/**/* src/**/*.rs]
  patterns = shared_files + (prebuilt_binary ? ['libexec/*'] : source_build_files)

  spec.files = Dir.glob(patterns, base: root).select { |path| File.file?(File.join(root, path)) }.sort
  spec.bindir = 'exe'
  spec.executables = ['sonicop']
  spec.require_paths = ['lib']

  if prebuilt_binary
    spec.platform = prebuilt_platform
  else
    spec.extensions = ['ext/sonicop/extconf.rb']
  end
end
