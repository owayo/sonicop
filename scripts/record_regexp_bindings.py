#!/usr/bin/env python3
"""実 Ruby 2.7.8 の捕捉名宣言を、一つのプロセス・独立した lambda で記録する。"""
import argparse
import tempfile
import hashlib
import itertools
import json
import pathlib
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--ruby27", required=True, type=pathlib.Path)
parser.add_argument("--include-character-escapes", action="store_true")
parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1] / "tests/fixtures/regexp_ruby27_bindings.json")
arguments = parser.parse_args()
temporary = tempfile.TemporaryDirectory(prefix="sonicop-ruby27-reference-")
directory = pathlib.Path(temporary.name)
atoms = ["'a'", "''", "()", "('b')", "(; 'c')"]
if arguments.include_character_escapes:
    atoms.append(r"?\C-\x43")
sources = []
for count in range(4):
    for items in itertools.product(atoms, repeat=count):
        for leading in (False, True):
            for trailing in (False, True):
                for depth in range(3):
                    expression = "; ".join(items)
                    if leading:
                        expression = "; " + expression
                    if trailing:
                        expression += ";"
                    for _ in range(depth):
                        expression = "(" + expression + ")"
                    sources.append("/(?<y>.)#{" + expression + "}/ =~ 'a'\n")

if arguments.include_character_escapes:
    for atom in [r"?\C-x", r"?\cx", r"?\C-u", r"?\cu", r"?\C-7", r"?\c7", r"?\M-x", r"?\M-u"]:
        header = "# encoding: ASCII-8BIT\n" if r"\M-" in atom else ""
        for expression in [atom, "(" + atom + ")", atom + "; 'a'"]:
            sources.append(header + "/(?<y>.)#{" + expression + "}/ =~ 'a'\n")
def ruby_quote(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"

harness = '''puts ["VERSION", RUBY_ENGINE, RUBY_VERSION, RUBY_PATCHLEVEL.to_s, RUBY_REVISION.to_s, RUBY_DESCRIPTION].join("\\t")
raise "outer y already bound" if defined?(y)
sources = [
''' + ",\n".join(ruby_quote(source) for source in sources) + '''
]
sources.each_with_index do |source, index|
  script = "-> {\\n" + source + "defined?(y)\\n}"
  script = script.b if source.start_with?("# encoding: ASCII-8BIT")
  value = eval(script).call
  raise "unexpected defined? result #{index}: #{value.inspect}" unless value.nil? || value == "local-variable"
  puts "#{index}\\t#{value == "local-variable" ? 1 : 0}"
  raise "y binding leaked at #{index}" if defined?(y)
end
'''
harness_path = directory / "runtime27-fold-audit.rb"
harness_path.write_text(harness, encoding="utf-8", newline="")
result = subprocess.run(
    [str(arguments.ruby27), "-W0", str(harness_path)],
    text=True, capture_output=True, timeout=55,
)
(directory / "runtime27-fold-audit.stdout").write_text(result.stdout, encoding="utf-8", newline="")
(directory / "runtime27-fold-audit.stderr").write_text(result.stderr, encoding="utf-8", newline="")
if result.returncode != 0:
    raise RuntimeError(f"reference failed with {result.returncode}: {result.stderr[:4000]}")
if result.stderr:
    raise RuntimeError(f"unexpected reference stderr: {result.stderr[:4000]}")
lines = result.stdout.splitlines()
version = lines.pop(0).split("\t")
assert version[0] == "VERSION" and version[2] == "2.7.8", version
assert len(lines) == len(sources), (len(lines), len(sources))
cases = []
for index, (source, line) in enumerate(zip(sources, lines)):
    row = line.split("\t")
    assert row[0] == str(index) and row[1] in ("0", "1"), (index, row)
    cases.append({"source": source, "declares_y": row[1] == "1"})
output = {
    "ruby": {
        "engine": version[1],
        "version": version[2],
        "patchlevel": int(version[3]),
        "revision": version[4],
        "description": version[5],
    },
    "cases": cases,
}
path = arguments.output
encoded = (json.dumps(output, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
path.write_bytes(encoded)
assert b"/home/" not in encoded and b"C:\\" not in encoded
print(json.dumps({
    "fixture": str(path),
    "cases": len(cases),
    "unique_sources": len({case["source"] for case in cases}),
    "declares_y": sum(case["declares_y"] for case in cases),
    "does_not_declare_y": sum(not case["declares_y"] for case in cases),
    "ruby": output["ruby"],
    "sha256": hashlib.sha256(encoded).hexdigest(),
}, ensure_ascii=False))