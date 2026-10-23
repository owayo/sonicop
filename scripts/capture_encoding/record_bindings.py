#!/usr/bin/env python3
"""自作の静的 regexp だけを使い、MRI の Symbol と whitequark の宣言を別に記録する。"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

SHAPES = [
    ("ascii", "(?<ascii>x)"),
    ("raw", "(?<あ>x)"),
    ("unicode", r"(?<\u3042>x)"),
    ("utf8bytes", r"(?<\xE3\x81\x82>x)"),
    ("eucbytes", r"(?<\xA4\xA2>x)"),
    ("sjisbytes", r"(?<\x82\xA0>x)"),
    ("ascii-body", "(?<ascii>あ)"),
    ("prefix-static", "#{'a'}(?<あ>x)"),
    ("capture-single", "#{'(?<あ>x)'}"),
    ("capture-single-escape", r"#{'(?<\u3042>x)'}"),
    ("capture-double-escape", r'#{"(?<\u3042>x)"}'),
    ("ascii-unicode", r"(?<\u0061>x)"),
]
ENCODINGS = [
    ("UTF-8", "utf-8"),
    ("ASCII-8BIT", "utf-8"),
    ("EUC-JP", "euc_jp"),
    ("Windows-31J", "cp932"),
    ("US-ASCII", "ascii"),
]
CODECS = {label: codec for label, codec in ENCODINGS}
CODECS["ASCII-8BIT"] = "latin1"
MRI = r"""
ARGV.each_with_index do |file, i|
  raw = File.binread(file)
  fields = [i]
  begin
    iseq = RubyVM::InstructionSequence.compile(raw, 'example.rb')
    locals = iseq.to_a[10].map { |n| s = n.to_s; s.encoding.name + ':' + s.unpack1('H*') }
    fields += ['ok', locals.join(','), '']
  rescue SyntaxError, ArgumentError => e
    fields += [e.class.name, '', e.message.unpack1('H*')]
  end
  begin
    # SHAPES は正規表現・静的文字列だけ。取得ソースのコードは実行しない。
    regexp = eval(raw.sub(/ =~ text\n\z/, "\n"), TOPLEVEL_BINDING, 'example.rb')
    names = regexp.names.map { |n| s = n.to_sym.to_s; s.encoding.name + ':' + s.unpack1('H*') }
    fields += ['ok', regexp.encoding.name, names.join(',')]
  rescue SyntaxError, ArgumentError, RegexpError => e
    fields += [e.class.name, '', '']
  end
  puts fields.join("\t")
end
"""
WHITE = r"""
require 'json'
gem 'rubocop', '= 1.89.0'
require 'rubocop'
$trace = []
def encoded(value)
  {encoding: value.encoding.name, hex: value.unpack1('H*')}
end
module CaptureEncodingRegexpTrace
  def static_regexp(parts, options)
    value = super
    $trace << {regexp: value && {source: encoded(value.source), encoding: value.encoding.name,
                                names: value.names.map { |n| encoded(n) }}}
    value
  end
end
module CaptureEncodingEnvTrace
  def declare(name)
    $trace << {declare: encoded(name.to_s)}
    super
  end
end
Parser::Builders::Default.prepend(CaptureEncodingRegexpTrace)
Parser::StaticEnvironment.prepend(CaptureEncodingEnvTrace)
rows = JSON.parse(File.read(ARGV[0]))
rows.each do |row|
  $trace = []
  begin
    ps = RuboCop::ProcessedSource.new([row['source_hex']].pack('H*'), 2.7, 'example.rb',
                                     parser_engine: :parser_whitequark)
    row['white27'] = {buffer_encoding: ps.buffer.source.encoding.name, ast: ps.ast&.type,
                     diagnostics: ps.diagnostics.map { |d| {level: d.level, reason: d.reason} },
                     trace: $trace}
  rescue Exception => e
    row['white27'] = {exception: e.class.name, trace: $trace}
  end
end
STDOUT.write(JSON.generate(rows))
"""


def symbols(text):
    return [dict(encoding=item.split(":")[0], hex=item.split(":")[1])
            for item in text.split(",") if item]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby27", required=True)
    parser.add_argument("--ruby27-arg", action="append", default=[])
    parser.add_argument("--ruby-reference", default="ruby")
    parser.add_argument("--output", type=Path,
                        default=Path(__file__).resolve().parents[2] /
                        "tests/fixtures/capture_binding_encodings.json")
    args = parser.parse_args()
    ruby27 = subprocess.run([args.ruby27, *args.ruby27_arg, "-e",
                             'puts [RUBY_VERSION, RUBY_PATCHLEVEL, RUBY_REVISION].join("\\t")'],
                            check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    runtime = ruby27.stdout.decode("ascii").strip().split("\t")
    assert runtime == ["2.7.8", "225", "1f4d4558484b370999954f3ede7e3aa3a3a01ef3"], runtime
    rows = []
    with tempfile.TemporaryDirectory(prefix="capture-encoding-") as temporary:
        directory = Path(temporary)
        paths = []
        for label, codec in ENCODINGS:
            for name, pattern in SHAPES:
                for flag in ["", "e", "s", "u", "n"]:
                    text = f"# encoding: {label}\n/{pattern}/{flag} =~ text\n"
                    try:
                        raw = text.encode(codec)
                    except UnicodeEncodeError:
                        continue
                    identifier = f"case-{len(rows):03}-{label.lower().replace('-', '')}-{name}-{flag or 'default'}"
                    path = directory / (identifier + ".rb")
                    path.write_bytes(raw)
                    paths.append(str(path))
                    pattern_bytes = raw.split(b"\n", 1)[1][1:].rsplit(b"/", 1)[0]
                    rows.append(dict(id=identifier, source=raw.decode(CODECS[label]),
                                     source_hex=raw.hex(), pattern_hex=pattern_bytes.hex(),
                                     flags=flag, encoding=label))
        assert len(rows) == 280
        result = subprocess.run([args.ruby27, *args.ruby27_arg, "-e", MRI, *paths],
                                check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        lines = result.stdout.decode("ascii").splitlines()
        assert len(lines) == len(rows)
        for line in lines:
            index, status, local_names, error_hex, re_status, re_encoding, re_names = line.split("\t")
            row = rows[int(index)]
            row["mri27"] = dict(status=status, names=symbols(local_names))
            if status != "ok":
                row["mri27"]["error_hex"] = error_hex
            row["referencable_names"] = [bytes.fromhex(n["hex"]).decode(CODECS[row["encoding"]])
                                          for n in row["mri27"]["names"]
                                          if bytes.fromhex(n["hex"]).isascii()
                                          or n["encoding"] == row["encoding"]]
            row["mri27_regexp"] = dict(id=row["id"], status=re_status,
                                         encoding=re_encoding, names=symbols(re_names))
            row["encoding_matched_names"] = [bytes.fromhex(n["hex"]).decode(CODECS[row["encoding"]])
                                               for n in symbols(re_names)
                                               if bytes.fromhex(n["hex"]).isascii()
                                               or n["encoding"] == row["encoding"]]
        inputs = directory / "inputs.json"
        inputs.write_text(json.dumps(rows, ensure_ascii=False), encoding="utf-8")
        result = subprocess.run([args.ruby_reference, "-e", WHITE, str(inputs)], check=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        rows = json.loads(result.stdout)
    fixture = {"ruby": {"version": "2.7.8", "patchlevel": 225,
                         "revision": "1f4d4558484b370999954f3ede7e3aa3a3a01ef3"},
               "rubocop_version": "1.89.0", "cases": rows}
    args.output.write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"recorded {len(rows)} independent MRI/whitequark cases")


if __name__ == "__main__":
    main()
