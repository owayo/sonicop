# frozen_string_literal: true

require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'

module ExternalOptionsReference
  ENV_KEYS = %w[RUBYOPT RUBYLIB RUBOCOP_OPTS RUBOCOP_TARGET_RUBY_VERSION].freeze

  def self.child_environment(case_options)
    # Gem・DLL 探索に必要な親環境を保ち、起動時の入力だけを子の copy から除く。
    environment = ENV.to_h
    environment.keys.each do |key|
      # Windows ENV はcase非依存だが、to_hは元spellingの通常Hash。ASCIIの4名だけ合わせる。
      compared = File::ALT_SEPARATOR == '\\' ? key.tr('a-z', 'A-Z') : key
      environment.delete(key) if ENV_KEYS.include?(compared)
    end
    environment['RUBOCOP_OPTS'] = case_options unless case_options.nil?
    environment
  end

  def self.capture(case_options, *command, **options)
    # Open3 の env Hash は既定では overlay。copy から除いた4キーを親から戻さない。
    Open3.capture3(child_environment(case_options), *command, **options.merge(unsetenv_others: true))
  end
end

if $PROGRAM_NAME == __FILE__
  # .rubocop は必ず binwrite し、File.read の変換と Shellwords の解釈を実 CLI で測る。
  cases = []
  { 'crlf' => "\r\n", 'cr' => "\r", 'vt' => "\v", 'ff' => "\f" }.each do |name, whitespace|
    cases << ["#{name}-delimiter", "--format#{whitespace}json#{whitespace}--stdin#{whitespace}example.rb"]
    cases << ["#{name}-single", "--format json --stdin 'a#{whitespace}日本.rb'"]
    cases << ["#{name}-double", "--format json --stdin \"a#{whitespace}日本.rb\""]
    cases << ["#{name}-escaped", "--format json --stdin a\\#{whitespace}日本.rb"]
  end
  cases.concat([
    ['empty-source', ''], ['empty-word', '--format json --stdin ""'],
    ['unicode', '--format json --stdin "日本 😀.rb"'], ['hash', '--format json --stdin #name.rb'],
    ['escaped-lf', "--format json --stdin a\\\nname.rb"],
    ['dq-escaped-lf', "--format json --stdin \"a\\\nname.rb\""],
    ['unmatched', "--format 'json"], ['nul', "--format json\0--stdin example.rb"]
  ])
  loader = 'gem "parser", "3.3.12.0"; gem "rubocop", "1.89.0"; load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
  records = []
  Dir.mktmpdir('sonicop-options-reference') do |directory|
    File.binwrite(File.join(directory, 'config.yml'), "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n")
    %w[dotfile environment].each do |origin|
      cases.each do |id, source|
        next if origin == 'environment' && source.include?("\0")

        dotfile = File.join(directory, '.rubocop')
        File.delete(dotfile) if File.exist?(dotfile)
        File.binwrite(dotfile, source) if origin == 'dotfile'
        arguments = %w[--config config.yml --cache false --only Layout/SpaceAroundOperators]
        arguments += %w[--format json --stdin example.rb] if source.empty?
        case_options = origin == 'environment' ? source : nil
        stdout, stderr, status = ExternalOptionsReference.capture(case_options, RbConfig.ruby, '-e', loader, '--', *arguments,
                                               stdin_data: "x=1\r\n", chdir: directory, binmode: true)
        report = begin
          JSON.parse(stdout).slice('files', 'summary')
        rescue JSON::ParserError
          nil
        end
        records << {
          'id' => "#{origin}-#{id}", 'origin' => origin, 'options_hex' => source.unpack1('H*'),
          'arguments' => arguments, 'stdin_hex' => "x=1\r\n".unpack1('H*'),
          'exit' => status.exitstatus, 'report' => report, 'stdout_empty' => stdout.empty?,
          'stderr_empty' => stderr.empty?
        }
      end
    end
  end
  STDOUT.binmode.write(JSON.pretty_generate({
    'ruby_version' => RUBY_VERSION, 'rubocop_version' => '1.89.0',
    'platform' => RUBY_PLATFORM, 'cases' => records
  }) + "\n")
end
