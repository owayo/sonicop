# frozen_string_literal: true

# Ruby の変換表を固定し、実行時・ビルド時の Ruby と外部 codec を不要にする。
# JIS X 0212 と変換先固有の別名は、WHATWG encoder の範囲に限定しない。
abort 'Ruby 4.0.8 is required' unless RUBY_VERSION == '4.0.8'
output = +"// 自動生成: ruby scripts/regexp_validation/generate_encoding_tables.rb\n"
output << "// Ruby 4.0.8 の String#encode。手編集せず、全 BMP の厳密な対応を記録する。\n\n"
[['EUC_JP', Encoding::EUC_JP], ['WINDOWS_31J', Encoding::Windows_31J]].each do |name, encoding|
  mapping = (0x80..0xFFFF).filter_map do |codepoint|
    next if (0xD800..0xDFFF).cover?(codepoint)
    begin
      bytes = codepoint.chr(Encoding::UTF_8).encode(encoding).bytes
    rescue Encoding::UndefinedConversionError
      next
    end
    abort 'expected at most three bytes' unless bytes.length.between?(1, 3)
    [codepoint, bytes.inject(0) { |acc, byte| (acc << 8) | byte }]
  end
  output << "pub(super) const #{name}: &[(u32, u32)] = &[\n"
  mapping.each { |point, packed| output << format("    (0x%X, 0x%X),\n", point, packed) }
  output << "];\n\n"
  # encode の別名が同じ byte になる場合も、decode の正本を Ruby 自身に確認する。
  decoded = mapping.map(&:last).uniq.select { |packed| packed >= 128 }.sort.map do |packed|
    bytes = [packed].pack('N').sub(/\A\x00+/, '').force_encoding(encoding)
    points = bytes.encode(Encoding::UTF_8).codepoints
    abort 'expected one decoded scalar' unless points.length == 1
    [packed, points[0]]
  end
  output << "pub(super) const #{name}_DECODE: &[(u32, u32)] = &[\n"
  decoded.each { |packed, point| output << format("    (0x%X, 0x%X),\n", packed, point) }
  output << "];\n\n"
end
output.sub!(/\n+\z/, "\n")
File.binwrite(ARGV[0] || File.expand_path('../../src/regexp_encoding_tables.rs', __dir__), output)
