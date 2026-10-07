#!/usr/bin/env ruby
# frozen_string_literal: true

# 本家 RuboCop と Sonicop の JSON を突き合わせ、cop 単位の一致を department 別に集計して
# README 用の表を出す。
#
# 一致の判定は各 run の offense の多重集合が完全に等しいこと。始終端の位置・文字数・
# メッセージ・severity・correctable を含め、重複も保持する。件数だけでは不足と過剰が相殺する。
#
# **コーパスが撃たなかった cop は「一致」にも「相違」にも数えない。** 発火しなかった cop は
# 沈黙が一致と見分けられないため、Exercised 列で測定の範囲を示す。ここを混ぜると、未実装の
# 設定値まで「一致」に化ける。
#
# 使い方:
#   ruby scripts/conformance_table.rb <spec.json>
#
# spec.json の形:
#   {
#     "cops": "<全 cop 名を 1 行 1 個で並べたファイル>",
#     "runs": [
#       {"label": "rubocop/rubocop", "root": "<コーパスのパス>",
#        "reference": "<本家の JSON>", "candidate": "<移植版の JSON>"}
#     ]
#   }
#
# 終了コード: 0=全 cop が発火して一致、1=差分または未発火、2=入力が不完全で未測定。

require 'json'

def load_offenses(path, root, into, run_id, cops)
  document = JSON.parse(File.read(path))
  raise ArgumentError, "#{path}: invalid report" unless document.is_a?(Hash)

  files = document.fetch('files')
  summary = document.fetch('summary')
  unless files.is_a?(Array) && summary.is_a?(Hash)
    raise ArgumentError, "#{path}: invalid files or summary"
  end
  inspected = summary.fetch('inspected_file_count')
  target = summary.fetch('target_file_count')
  # 本家は例外で中断しても整形式の JSON を出すため、offense の比較より先に検査完了を確認する。
  unless inspected.is_a?(Integer) && inspected.positive? && inspected == target && files.length == inspected
    raise ArgumentError, "#{path}: incomplete inspection (inspected=#{inspected}, target=#{target}, files=#{files.length})"
  end

  paths = files.map do |file|
    unless file.is_a?(Hash) && file['path'].is_a?(String) && file['offenses'].is_a?(Array)
      raise ArgumentError, "#{path}: invalid file record"
    end
    relative = file.fetch('path').sub(%r{\A#{Regexp.escape(root)}/}, '').sub(%r{\A\./}, '')
    file.fetch('offenses').each do |offense|
      location = offense.is_a?(Hash) && offense['location']
      unless location.is_a?(Hash) && %w[line column last_line last_column length].all? { |field| location[field].is_a?(Integer) } &&
             %w[cop_name message severity].all? { |field| offense[field].is_a?(String) } &&
             [true, false].include?(offense['correctable'])
        raise ArgumentError, "#{path}: invalid offense fields"
      end
      cop = offense.fetch('cop_name')
      raise ArgumentError, "#{path}: unlisted cop #{cop}" unless cops.include?(cop)

      # 同名の相対パスでも別 run の不足と過剰は相殺させない。length は文字数なので終端座標と別に比較する。
      key = [run_id, relative, location.fetch('line'), location.fetch('column'), location.fetch('last_line'),
             location.fetch('last_column'), location.fetch('length'), offense.fetch('message'),
             offense.fetch('severity'), offense.fetch('correctable')]
      into[cop][key] += 1
    end
    relative
  end
  raise ArgumentError, "#{path}: duplicate target paths" unless paths.uniq.length == paths.length

  paths.sort
end

def main(spec_path)
  raise ArgumentError, "missing spec path" unless spec_path.is_a?(String)

  spec = JSON.parse(File.read(spec_path))
  raise ArgumentError, "invalid spec" unless spec.is_a?(Hash) && spec["cops"].is_a?(String)
  cops = File.readlines(spec.fetch('cops')).map(&:strip).reject(&:empty?)
  raise ArgumentError, 'no cops' if cops.empty?
  raise ArgumentError, 'duplicate cop names' unless cops.uniq.length == cops.length

  runs = spec.fetch('runs')
  raise ArgumentError, 'no runs' unless runs.is_a?(Array) && !runs.empty?

  reference = Hash.new { |hash, key| hash[key] = Hash.new(0) }
  candidate = Hash.new { |hash, key| hash[key] = Hash.new(0) }
  cop_names = cops.to_h { |cop| [cop, true] }
  runs.each_with_index do |run, run_id|
    raise ArgumentError, "run #{run_id}: invalid root" unless run.is_a?(Hash) && run['root'].is_a?(String)

    unless %w[reference candidate].all? { |key| run[key].is_a?(String) }
      raise ArgumentError, "run #{run_id}: invalid report path"
    end

    a = load_offenses(run.fetch('reference'), run.fetch('root'), reference, run_id, cop_names)
    b = load_offenses(run.fetch('candidate'), run.fetch('root'), candidate, run_id, cop_names)
    raise ArgumentError, "run #{run_id}: different target paths" unless a == b
  end

  rows = cops.group_by { |cop| cop.split('/').first }.sort.map do |department, names|
    exercised = names.reject { |cop| reference[cop].empty? && candidate[cop].empty? }
    exact = exercised.count { |cop| reference[cop] == candidate[cop] }
    [department, names.size, exercised.size, exact, exercised.size - exact]
  end

  total = rows.transpose[1..].map(&:sum)
  puts '| Department | Cops | Exercised | Exact match | Diverging |'
  puts '|---|---:|---:|---:|---:|'
  rows.each do |department, count, exercised, exact, diverging|
    mark = exact == count ? " **#{exact} ✓**" : " #{exact}"
    puts "| #{department} | #{count} | #{exercised} |#{mark} | #{diverging} |"
  end
  puts "| **Total** | **#{total[0]}** | **#{total[1]}** | **#{total[2]}** | **#{total[3]}** |"

  diverging = cops.reject { |cop| reference[cop].empty? && candidate[cop].empty? }
                  .reject { |cop| reference[cop] == candidate[cop] }
  unless diverging.empty?
    warn ''
    warn 'Diverging cops:'
    diverging.each do |cop|
      a = reference[cop]
      b = candidate[cop]
      difference = (a.keys | b.keys).sum { |key| (a[key] - b[key]).abs }
      warn format('  %-45s %d', cop, difference)
    end
  end
  total[0] == total[2] ? 0 : 1
rescue JSON::ParserError, KeyError, ArgumentError, SystemCallError => e
  warn "Unmeasured: #{e.message}"
  2
end

exit main(ARGV[0])
