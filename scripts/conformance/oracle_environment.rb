# frozen_string_literal: true

module SonicopOracleEnvironment
  INJECTION_KEYS = %w[RUBYOPT RUBYLIB RUBOCOP_OPTS RUBOCOP_TARGET_RUBY_VERSION].freeze

  def self.without_injections
    # WindowsのENV[]は大小を区別しないが、ENV.to_hは実キーの綴りを保持する。
    # 実在するキーを保存し、GEM/PATH/OS等の他ENVには触れない。
    previous = ENV.to_h.select { |key, _| injection_key?(key) }
    previous.each_key { |key| ENV.delete(key) }
    yield
  ensure
    # 呼出し中に作られた別綴りも先に除き、元の綴り・空文字・不存在を復元する。
    ENV.keys.each { |key| ENV.delete(key) if injection_key?(key) }
    previous.each { |key, value| ENV[key] = value }
  end

  def self.injection_key?(key)
    key = key.tr('a-z', 'A-Z') if RUBY_PLATFORM.match?(/mingw|mswin/) && key.ascii_only?
    INJECTION_KEYS.include?(key)
  end
  private_class_method :injection_key?
end
