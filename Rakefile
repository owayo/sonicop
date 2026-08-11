# frozen_string_literal: true

require 'rake'
require 'rake/testtask'

require_relative 'script/sonicop_version'

# 入口は Makefile に一本化してある。ここは gem 配布とバージョン整合に固有の
# タスクだけを持ち、cargo を叩く処理は Makefile へ委譲する。

namespace :version do
  desc 'Regenerate lib/sonicop/version.rb from Cargo.toml'
  task :sync do
    if SonicopVersion.sync!
      puts "lib/sonicop/version.rb -> #{SonicopVersion.gem_version}"
    else
      puts "lib/sonicop/version.rb is up to date (#{SonicopVersion.gem_version})"
    end
  end

  desc 'Set VERSION across Cargo.toml, Cargo.lock and lib/sonicop/version.rb'
  task :set do
    version = ENV.fetch('VERSION', nil)
    abort 'VERSION is required' if version.nil? || version.empty?

    SonicopVersion.set!(version)
    puts "version -> #{SonicopVersion.cargo_version}"
  rescue SonicopVersion::Mismatch => error
    abort error.message
  end

  desc 'Fail when Cargo.toml and lib/sonicop/version.rb disagree'
  task :check do
    puts "sonicop #{SonicopVersion.check!}"
  rescue SonicopVersion::Mismatch => error
    abort error.message
  end
end

namespace :test do
  Rake::TestTask.new(:ruby) do |task|
    task.description = 'Run the Ruby wrapper tests'
    task.libs = %w[lib test]
    task.test_files = FileList['test/**/*_test.rb']
    task.warning = false
    task.verbose = false
  end
end

desc 'Build the source gem'
task gem: :'version:check' do
  sh 'gem build sonicop.gemspec'
end

namespace :gem do
  desc 'Build a platform gem (BINARY and GEM_PLATFORM are required)'
  task platform: :'version:check' do
    binary = ENV['BINARY']
    platform = ENV['GEM_PLATFORM']
    abort 'BINARY and GEM_PLATFORM are required' unless binary && platform

    ruby 'script/package_gem', binary, platform
  end
end

desc 'Run every local quality gate (delegates to make)'
task :check do
  sh 'make check'
end

task default: :check
