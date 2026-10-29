# frozen_string_literal: true

# 制御はNative所有のprivate directoryへ分離し、拡張のSTDIN/STDOUT/STDERRを奪わない。
require 'json'
require 'yaml'
require 'stringio'

module SonicopRuntime
  class << self
    attr_accessor :baseline, :full_override

    def copy(value)
      Marshal.load(Marshal.dump(value))
    end

    # getterを新しく呼ばず、RuboCop自身が初めて既定値を使った時点だけを記録する。
    def remember_default(config)
      self.baseline ||= copy(config.to_h) unless full_override
      config
    end

    def changes(before, after, path = [])
      if before.is_a?(Hash) && after.is_a?(Hash)
        (before.keys | after.keys).flat_map do |key|
          next [{ 'path' => path + [key], 'remove' => true }] unless after.key?(key)
          next changes(before[key], after[key], path + [key]) if before.key?(key)

          [{ 'path' => path + [key], 'yaml' => YAML.dump(after[key]) }]
        end
      elsif before == after
        []
      else
        [{ 'path' => path, 'yaml' => YAML.dump(after) }]
      end
    end

    def snapshot
      registry = RuboCop::Cop::Registry.global
      current_default = RuboCop::ConfigLoader.instance_variable_get(:@default_configuration)
      {
        'names' => registry.names,
        'departments' => registry.departments.map(&:to_s),
        'loaded_features' => RuboCop::ConfigLoader.loaded_features.to_a,
        'loaded_plugins' => RuboCop::ConfigLoader.loaded_plugins.map { |plugin| plugin.about.name }.grep(String),
        'defaults' => if full_override && current_default
                        [{ 'path' => [], 'yaml' => YAML.dump(current_default.to_h) }]
                      elsif baseline && current_default
                        changes(baseline, current_default.to_h)
                      else
                        []
                      end
      }
    end

    def exception(error)
      kind = if error.is_a?(SystemExit)
               'exit'
             elsif error.is_a?(Interrupt)
               'interrupt'
             elsif defined?(RuboCop::Cop::AmbiguousCopName) && error.is_a?(RuboCop::Cop::AmbiguousCopName)
               'ambiguous'
             elsif defined?(RuboCop::ConfigNotFoundError) && error.is_a?(RuboCop::ConfigNotFoundError) || (defined?(RuboCop::IncorrectCopNameError) && error.is_a?(RuboCop::IncorrectCopNameError)) || (defined?(RuboCop::OptionArgumentError) && error.is_a?(RuboCop::OptionArgumentError))
               'argument'
             elsif defined?(RuboCop::Error) && error.is_a?(RuboCop::Error)
               'rubocop'
             else
               'plain'
             end
      status = error.is_a?(SystemExit) ? error.status : (error.is_a?(Interrupt) ? 130 : nil)
      { 'class' => error.class.name, 'message' => error.message, 'kind' => kind, 'status' => status }
    end

    def extensions(request)
      # ConfigLoaderResolverとOptionsを使い、dot相対とCLI plain requireを混同しない。
      if request['cli']
        options = RuboCop::Options.new
        request.fetch('entries').each do |entry|
          options.send(entry.fetch('plugin') ? :plugin_feature : :require_feature, entry.fetch('name'))
        end
      else
        hash = YAML.respond_to?(:unsafe_load) ? YAML.unsafe_load(request.fetch('yaml')) : YAML.load(request.fetch('yaml'))
        path = request.fetch('origin')
        config = RuboCop::Config.create(hash, path, check: false)
        resolver = RuboCop::ConfigLoaderResolver.new
        plugins = resolver.resolve_plugins(config, hash.delete('plugins'))
        RuboCop::ConfigLoader.add_loaded_plugins(plugins)
        features = resolver.resolve_requires(path, hash)
        RuboCop::ConfigLoader.add_loaded_features(features)
      end
      { 'snapshot' => snapshot }
    end

    def namespace(request)
      warnings = StringIO.new
      previous = $stderr
      result = { 'qualified' => [], 'warnings' => [] }
      begin
        # 本当のnamespace到達点でだけgetterを使い、unqualified cacheを先に初期化しない。
        $stderr = warnings
        request.fetch('names').each do |name|
          result['qualified'] << RuboCop::Cop::Registry.qualified_cop_name(name, request.fetch('origin'))
        end
      rescue Exception => error # rubocop:disable Lint/RescueException
        result['error'] = exception(error)
      ensure
        $stderr = previous
        result['warnings'] = warnings.string.lines
      end
      result['snapshot'] = snapshot
      result
    end

    def handle(request)
      Rainbow.enabled = request['color'] unless request['color'].nil?
      original = $stdout
      $stdout = $stderr if request['redirect']
      case request.fetch('action')
      when 'extensions' then extensions(request)
      when 'namespace' then namespace(request)
      when 'snapshot' then { 'snapshot' => snapshot }
      when 'default'
        RuboCop::ConfigLoader.default_configuration
        { 'snapshot' => snapshot }
      when 'stdin' then { 'bytes' => $stdin.binmode.read.bytes }
      else raise ArgumentError, 'unknown native runtime request'
      end
    rescue Exception => error # rubocop:disable Lint/RescueException
      { 'error' => exception(error) }
    ensure
      $stdout = original
      STDOUT.flush
      STDERR.flush
    end

    def write(path, value)
      # 応答はclose後にrenameし、途中JSONをNativeが完了と誤認しない。
      temporary = path + '.pending'
      File.open(temporary, File::WRONLY | File::CREAT | File::EXCL) do |file|
        file.binmode
        file.write(JSON.generate(value))
      end
      File.rename(temporary, path)
    end
  end
end

control = ARGV.shift
STDOUT.sync = true
STDERR.sync = true
bootstrap_error = nil
begin
  load_path = File.join(control, 'load-path.json')
  $LOAD_PATH.replace(JSON.parse(File.binread(load_path))) if File.file?(load_path)
  if ENV['SONICOP_BUNDLER_VERSION']
    gem 'bundler', ENV.fetch('SONICOP_BUNDLER_VERSION')
    require 'bundler/setup'
  end
  gem 'rubocop', '1.89.0'
  require 'rubocop'
  RuboCop::ConfigLoader.clear_options
  current_default = RuboCop::ConfigLoader.instance_variable_get(:@default_configuration)
  SonicopRuntime.remember_default(current_default) if current_default
  RuboCop::ConfigLoader.singleton_class.prepend(Module.new do
    def default_configuration
      SonicopRuntime.remember_default(super)
    end

    def default_configuration=(config)
      SonicopRuntime.full_override = true unless SonicopRuntime.baseline
      super
    end
  end)
  RuboCop::ConfigLoader.default_configuration if ENV['SONICOP_RUNTIME_SEED_DEFAULT'] == '1'
  # 通常Nativeが以前に訪問したnamespaceのcache時点だけを遡って再現する。
  RuboCop::Cop::Registry.global.unqualified_cop_names if ENV['SONICOP_RUNTIME_SEED_UNQUALIFIED'] == '1' 
rescue Exception => error # rubocop:disable Lint/RescueException
  # 失敗もNativeの最終error表示後まで生存させ、at_exitをその前に実行しない。
  bootstrap_error = SonicopRuntime.exception(error)
end
SonicopRuntime.write(File.join(control, 'boot.json'), bootstrap_error ? { 'error' => bootstrap_error } : { 'ready' => true })
sequence = 0
loop do
  sequence += 1
  path = File.join(control, "request-#{sequence}.json")
  sleep(0.005) until File.file?(path)
  request = JSON.parse(File.binread(path))
  if request['action'] == 'close'
    SonicopRuntime.write(File.join(control, "response-#{sequence}.json"), { 'closing' => true })
    exit(request.fetch('status', 0))
  end

  response = bootstrap_error ? { 'error' => bootstrap_error } : SonicopRuntime.handle(request)
  SonicopRuntime.write(File.join(control, "response-#{sequence}.json"), response)
end
