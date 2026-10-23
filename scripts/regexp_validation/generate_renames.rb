# frozen_string_literal: true

require 'open3'
# native ABI の一覧から作る開発用コマンド。配布物のビルドは nm を実行しない。
text, status = Open3.capture2('nm','-g','--defined-only',ARGV.fetch(0))
abort 'nm failed' unless status.success?
names = text.lines.filter_map do |line|
  fields = line.split
  next unless fields.length >= 3 && %w[T D B R C W].include?(fields[-2])
  name = fields[-1].delete_prefix('sonicop_onigmo_native_')
  name if name.start_with?('onig','Onig','st_')
end.uniq.sort
abort 'no Onigmo ABI found' if names.empty?
header = +"/* 生成: 公開 ABI の名前を固定し、他の Ruby/Onigmo と同一 process で衝突させない。 */\n"
header << names.map { |name| "#define #{name} sonicop_onigmo_native_#{name}\n" }.join
File.binwrite(File.expand_path('../../crates/sonicop-onigmo-compat/rename.h',__dir__),header)
