#!/usr/bin/env ruby
# frozen_string_literal: true

# 本家 RuboCop と Sonicop の JSON を突き合わせ、cop 単位の一致を department 別に集計して
# README 用の表を出す。
#
# 一致の判定は offense の集合が完全に等しいこと -- 位置 (行・列)、メッセージ、severity、
# correctable まで含めて 1 件も違わないこと。件数の一致では過剰と不足が相殺されるので使わない。
#
# **コーパスが撃たなかった cop は「一致」にも「相違」にも数えない。** 発火しなかった cop は
# 沈黙が一致と見分けられないため、別列 (Not exercised) に置く。ここを混ぜると、実装していない
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

require 'json'

def load_offenses(path, root, into)
  document = JSON.parse(File.read(path))
  document['files'].each do |file|
    relative = file['path'].sub(%r{\A#{Regexp.escape(root)}/}, '').sub(%r{\A\./}, '')
    file['offenses'].each do |offense|
      location = offense['location']
      key = [relative, location['line'], location['column'], location['last_line'],
             location['last_column'], offense['message'], offense['severity'],
             offense['correctable']]
      into[offense['cop_name']][key] += 1
    end
  end
end

spec = JSON.parse(File.read(ARGV.fetch(0)))
cops = File.readlines(spec.fetch('cops')).map(&:strip).reject(&:empty?)

reference = Hash.new { |hash, key| hash[key] = Hash.new(0) }
candidate = Hash.new { |hash, key| hash[key] = Hash.new(0) }
spec.fetch('runs').each do |run|
  load_offenses(run.fetch('reference'), run.fetch('root'), reference)
  load_offenses(run.fetch('candidate'), run.fetch('root'), candidate)
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
  mark = diverging.zero? && exercised.positive? ? " **#{exact} ✓**" : " #{exact}"
  puts "| #{department} | #{count} | #{exercised} |#{mark} | #{diverging} |"
end
puts "| **Total** | **#{total[0]}** | **#{total[1]}** | **#{total[2]}** | **#{total[3]}** |"

diverging = cops.reject { |cop| reference[cop].empty? && candidate[cop].empty? }
                .reject { |cop| reference[cop] == candidate[cop] }
return if diverging.empty?

warn ''
warn 'Diverging cops:'
diverging.each do |cop|
  a = reference[cop]
  b = candidate[cop]
  difference = (a.keys | b.keys).sum { |key| (a[key] - b[key]).abs }
  warn format('  %-45s %d', cop, difference)
end
