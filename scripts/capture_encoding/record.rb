# frozen_string_literal: true

require 'json'
require 'tmpdir'
require 'open3'
require 'digest'
require 'rbconfig'

gem 'rubocop', '1.89.0'
require 'rubocop/version'
abort 'Ruby 4.0.8 is required' unless RUBY_VERSION == '4.0.8'
root = File.expand_path('../..', __dir__)
cases_path = File.join(__dir__, 'cases.json')
original = JSON.parse(File.binread(cases_path))
cases = original.each_with_object({}) do |(name, hex), expanded|
  source = [hex].pack('H*')
  expanded["#{name}-lf.rb"] = source.unpack1('H*')
  expanded["#{name}-crlf.rb"] = source.gsub("\n", "\r\n").unpack1('H*')
end
settings = [['2.7', nil], ['2.7', 'parser_whitequark'], ['3.0', 'parser_whitequark'], ['3.1', 'parser_whitequark'],
            ['3.2', 'parser_whitequark'], ['3.3', 'parser_whitequark'], ['3.4', 'parser_whitequark'],
            ['3.3', nil], ['3.3', 'parser_prism'], ['3.4', 'parser_prism']]
fixture = {rubocop_version: RuboCop::Version::STRING, ruby_version: RUBY_VERSION,
           inputs_sha256: Digest::SHA256.file(cases_path).hexdigest, only: 'Lint/Syntax', cases: cases, groups: []}
Dir.mktmpdir('sonicop-capture-encoding-oracle') do |directory|
  settings.each do |version, engine|
    config = "AllCops:\n  TargetRubyVersion: #{version}\n  NewCops: disable\n"
    config << "  ParserEngine: #{engine}\n" if engine
    File.binwrite(File.join(directory,'.rubocop.yml'),config)
    group = {target: version, engine: engine, config: config, runs: []}
    [nil, '-a', '-A'].each do |mode|
      cases.each { |name, hex| File.binwrite(File.join(directory,name),[hex].pack('H*')) }
      args = ['rubocop','_1.89.0_','--cache','false','--config','.rubocop.yml','--only',fixture[:only],'--format','json']
      args << mode if mode
      args.concat(cases.keys.sort)
      stdout, stderr, status = Open3.capture3(*args,chdir: directory)
      parsed = JSON.parse(stdout)
      abort "incomplete upstream run: #{version} #{engine} #{mode}: #{stderr}" unless parsed.dig('summary','inspected_file_count') == cases.length
      abort "upstream internal error: #{version} #{engine} #{mode}: #{stderr}" unless [0,1].include?(status.exitstatus)
      parsed.fetch('files').each { |file| file['path'] = File.basename(file['path']) }
      actual_names = parsed.fetch('files').map { |file| file.fetch('path') }
      complete = parsed.dig('summary', 'target_file_count') == cases.length &&
                 actual_names.length == cases.length && actual_names.uniq.length == cases.length &&
                 actual_names.sort == cases.keys.sort
      abort "incomplete upstream path set: #{version} #{engine} #{mode}" unless complete
      parsed['files'].sort_by! { |file| file['path'] }
      written = cases.keys.to_h { |name| [name,File.binread(File.join(directory,name)).unpack1('H*')] }
      group[:runs] << {mode: mode, exit: status.exitstatus, files: parsed.fetch('files'), summary: parsed.fetch('summary'), written: written}
      warn "recorded target=#{version} engine=#{engine || 'default'} mode=#{mode || 'inspect'} files=#{cases.length}"
    end
    fixture[:groups] << group
  end
end
File.binwrite(File.join(root,'tests/fixtures/capture_encoding.json'),JSON.pretty_generate(fixture)+"\n")
