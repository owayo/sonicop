# frozen_string_literal: true

require_relative 'test_helper'
require_relative '../scripts/conformance/oracle_environment'
require 'json'
require 'open3'

class OracleEnvironmentTest < Minitest::Test
  def with_environment(values)
    original = ENV.to_h
    values.each { |key, value| value.nil? ? ENV.delete(key) : ENV[key] = value }
    yield
  ensure
    ENV.replace(original)
  end

  def test_normal_return_preserves_other_environment_and_restores_values
    values = SonicopOracleEnvironment::INJECTION_KEYS.to_h { |key| [key, "poison-#{key}"] }
    values.merge!(%w[GEM_HOME GEM_PATH PATH SYSTEMROOT].to_h { |key| [key, "keep-#{key}"] })
    # POSIXで同名の小文字keyは注入元ではないため、通常のENVとして保持する。
    unless RUBY_PLATFORM.match?(/mingw|mswin/)
      values.merge!(SonicopOracleEnvironment::INJECTION_KEYS.to_h { |key| [key.downcase, 'keep-lowercase'] })
    end
    with_environment(values) do
      before = ENV.to_h
      result = SonicopOracleEnvironment.without_injections do
        # 4key以外の全ENVを比較し、保持対象の取りこぼしも拒否する。
        assert_equal before.reject { |key, _| SonicopOracleEnvironment::INJECTION_KEYS.include?(key) }, ENV.to_h
        :complete
      end
      assert_equal :complete, result
      assert_equal before, ENV.to_h
    end
  end

  def test_exception_restores_the_parent_even_if_the_block_changes_injections
    values = SonicopOracleEnvironment::INJECTION_KEYS.to_h { |key| [key, "parent-#{key}"] }
    with_environment(values) do
      before = ENV.to_h
      assert_raises(RuntimeError) do
        SonicopOracleEnvironment.without_injections do
          assert SonicopOracleEnvironment::INJECTION_KEYS.none? { |key| ENV.key?(key) }
          SonicopOracleEnvironment::INJECTION_KEYS.each { |key| ENV[key] = 'changed-in-block' }
          raise 'reference failed'
        end
      end
      assert_equal before, ENV.to_h
    end
  end

  def test_unset_and_empty_values_are_restored_separately
    with_environment('RUBYOPT' => nil, 'RUBYLIB' => '', 'RUBOCOP_OPTS' => nil, 'RUBOCOP_TARGET_RUBY_VERSION' => '') do
      before = ENV.to_h
      SonicopOracleEnvironment.without_injections do
        assert SonicopOracleEnvironment::INJECTION_KEYS.none? { |key| ENV.key?(key) }
        ENV['RUBYOPT'] = 'temporary'
        ENV['RUBYLIB'] = 'temporary'
      end
      assert_equal before, ENV.to_h
      refute ENV.key?('RUBYOPT')
      refute ENV.key?('RUBOCOP_OPTS')
      assert_equal '', ENV['RUBYLIB']
      assert_equal '', ENV['RUBOCOP_TARGET_RUBY_VERSION']
    end
  end

  def test_windows_actual_key_spelling_survives_return_and_exception_in_real_ruby
    skip 'WindowsのENV APIの実キー綴りを検査する' unless RUBY_PLATFORM.match?(/mingw|mswin/)

    code = <<~'RUBY'
      require ARGV.shift
      mode, spelling = ARGV
      canonical = SonicopOracleEnvironment::INJECTION_KEYS
      selected = lambda do |values|
        values.select { |key, _| key.ascii_only? && canonical.include?(key.tr('a-z', 'A-Z')) }
      end
      names = if spelling == 'lower'
                %w[rubyopt rubylib rubocop_opts rubocop_target_ruby_version]
              else
                %w[RubyOpt ruByLIB RuboCop_OpTs RuBoCoP_tArGeT_rUbY_vErSiOn]
              end
      names.zip(['parent-option', '', 'parent-cops', 'parent-target']).each { |key, value| ENV[key] = value }
      before = ENV.to_h
      before_selected = selected.call(before)
      inside = nil
      returned = nil
      raised = false
      failure = Class.new(StandardError)
      begin
        returned = SonicopOracleEnvironment.without_injections do
          inside = ENV.to_h
          # 呼出し中に再導入された別綴りを消してから元の実キーへ戻す必要がある。
          %w[rUbYoPt RuByLiB rubocop_opts RUBOCOP_TARGET_RUBY_VERSION].each { |key| ENV[key] = 'changed-in-block' }
          raise failure if mode == 'exception'
          :complete
        end
      rescue failure
        raised = true
      end
      after = ENV.to_h
      result = {
        mode: mode, spelling: spelling,
        before: before_selected, after: selected.call(after),
        actual_spelling_measured: before_selected.keys.sort == names.sort,
        injections_absent_inside: selected.call(inside).empty?,
        other_environment_preserved: before.reject { |key, _| before_selected.key?(key) } == inside,
        exact_environment_restored: before == after,
        control_flow_preserved: mode == 'exception' ? raised : returned == :complete
      }
      success = result.values_at(:actual_spelling_measured, :injections_absent_inside,
        :other_environment_preserved, :exact_environment_restored, :control_flow_preserved).all?
      STDOUT.write(JSON.generate(result))
      exit(success ? 0 : 1)
    RUBY
    # 起動前のRUBYOPT/LIBも、親の実際の大小表記から除外して実Rubyを起動する。
    child_env = ENV.to_h.reject do |key, _|
      key.ascii_only? && SonicopOracleEnvironment::INJECTION_KEYS.include?(key.tr('a-z', 'A-Z'))
    end
    helper = File.expand_path('../scripts/conformance/oracle_environment.rb', __dir__)
    results = %w[normal exception].product(%w[lower mixed]).map do |mode, spelling|
      stdout, stderr, status = Open3.capture3(
        child_env, RbConfig.ruby, '-rjson', '-e', code, helper, mode, spelling, unsetenv_others: true
      )
      JSON.parse(stdout).merge('process_success' => status.success?, 'stderr' => stderr)
    end
    failures = results.reject { |result| result['process_success'] && result['stderr'].empty? }
    assert_empty failures, JSON.pretty_generate(results)
  end
end
