#!/usr/bin/env python3
"""MRI 2.7.8 と Parser::Ruby27 の literal leaf を、それぞれ 1 プロセスで採取する。"""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def ruby_quote(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"


def inputs():
    rows = []
    def add(name, literal):
        for encoding, header in [
            ("utf8", "# encoding: UTF-8\n"),
            ("binary", "# encoding: ASCII-8BIT\n"),
        ]:
            rows.append({"id": name + "-" + encoding, "source": header + literal + "\n"})

    focused = [
        ("meta_raw_x", r"\M-x"), ("meta_raw_u", r"\M-u"), ("meta_raw_7", r"\M-7"),
        ("control_raw_x", r"\C-x"), ("control_raw_u", r"\C-u"), ("control_raw_7", r"\C-7"),
        ("shortcontrol_raw_x", r"\cx"), ("shortcontrol_raw_u", r"\cu"), ("shortcontrol_raw_7", r"\c7"),        ("meta_hex43", r"\M-\x43"), ("meta_hex5a", r"\M-\x5a"),
        ("meta_hexff", r"\M-\xff"), ("meta_hex4", r"\M-\x4"),
        ("meta_octal103", r"\M-\103"), ("meta_octal7", r"\M-\7"),
        ("meta_ascii", r"\M-C"), ("meta_question", r"\M-?"),
        ("ctrl_hex43", r"\C-\x43"), ("short_ctrl_hex43", r"\c\x43"),
        ("ctrl_hex5a", r"\C-\x5a"), ("short_ctrl_hex5a", r"\c\x5a"),
        ("ctrl_hexff", r"\C-\xff"), ("short_ctrl_hexff", r"\c\xff"),
        ("ctrl_hex80", r"\C-\x80"), ("short_ctrl_hex80", r"\c\x80"),
        ("ctrl_hex7f", r"\C-\x7f"), ("short_ctrl_hex7f", r"\c\x7f"),
        ("ctrl_hex4", r"\C-\x4"), ("short_ctrl_hex4", r"\c\x4"),
        ("ctrl_octal103", r"\C-\103"), ("short_ctrl_octal103", r"\c\103"),
        ("ctrl_octal7", r"\C-\7"), ("short_ctrl_octal7", r"\c\7"),
        ("meta_ctrl_ascii", r"\M-\C-a"), ("ctrl_meta_ascii", r"\C-\M-a"),
        ("meta_shortctrl_ascii", r"\M-\ca"), ("shortctrl_meta_ascii", r"\c\M-a"),
        ("meta_ctrl_question", r"\M-\C-?"), ("ctrl_meta_question", r"\C-\M-?"),
        ("meta_shortctrl_question", r"\M-\c?"), ("shortctrl_meta_question", r"\c\M-?"),
        ("meta_ctrl_hex", r"\M-\C-\x43"), ("ctrl_meta_hex", r"\C-\M-\x43"),
        ("meta_shortctrl_hex", r"\M-\c\x43"), ("shortctrl_meta_hex", r"\c\M-\x43"),
        ("meta_ctrl_octal", r"\M-\C-\103"), ("ctrl_meta_octal", r"\C-\M-\103"),
        ("meta_shortctrl_octal", r"\M-\c\103"), ("shortctrl_meta_octal", r"\c\M-\103"),
        ("meta_twice", r"\M-\M-a"), ("ctrl_twice", r"\C-\C-a"),
        ("bell", r"\a"), ("backspace", r"\b"), ("escape", r"\e"),
        ("formfeed", r"\f"), ("newline", r"\n"), ("return", r"\r"),
        ("space", r"\s"), ("tab", r"\t"), ("vertical_tab", r"\v"),
        ("backslash", r"\\"), ("double_quote", r'\"'), ("single_quote", r"\'"),
        ("hex0", r"\x0"), ("hex00", r"\x00"), ("hex43", r"\x43"),
        ("hex7f", r"\x7f"), ("hex80", r"\x80"), ("hexff", r"\xff"),
        ("octal0", r"\0"), ("octal7", r"\7"), ("octal10", r"\10"),
        ("octal77", r"\77"), ("octal103", r"\103"),
        ("octal377", r"\377"), ("octal400", r"\400"), ("octal777", r"\777"),
        ("unicode_ascii", r"\u0041"), ("unicode_latin", r"\u00e9"),
        ("unicode_braced", r"\u{e9}"), ("unicode_emoji", r"\u{1f600}"),
        ("unicode_multiple", r"\u{41 e9}"), ("byte_unicode", r"\x43\u00e9"),
        ("raw_ascii", "a"), ("raw_latin", "é"), ("raw_japanese", "あ"),
        ("raw_emoji", "😀"), ("escaped_raw_latin", "\\é"), ("unknown_escape", r"\q"),
        ("invalid_meta_end", "\\M-"), ("invalid_ctrl_end", "\\C-"),
        ("invalid_shortctrl_end", "\\c"), ("invalid_hex", r"\x"),
        ("invalid_meta_hex", r"\M-\x"), ("invalid_ctrl_hex", r"\C-\x"),
        ("invalid_unicode", r"\u"), ("invalid_unicode_high", r"\u{110000}"),
        ("invalid_unicode_surrogate", r"\uD800"),
    ]
    for name, content in focused:
        add("double_" + name, '"' + content + '"')
        add("percent_double_" + name, "%Q{" + content + "}")
        add("character_" + name, "?" + content)

    delimiters = [("paren", "(", ")"), ("bracket", "[", "]"),
                  ("brace", "{", "}"), ("angle", "<", ">"),
                  ("bar", "|", "|"), ("bang", "!", "!"), ("slash", "/", "/")]
    for style in ("q", "Q"):
        for label, opening, closing in delimiters:
            bodies = [
                ("plain", "a"), ("raw_unicode", "é"),
                ("backslash", r"\\"), ("opening", "\\" + opening),
                ("closing", "\\" + closing),
                ("both", "\\" + opening + "a" + "\\" + closing),
                ("hex_literal", r"\x43"), ("unicode_literal", r"\u00e9"),
                ("meta_literal", r"\M-C"), ("unknown_literal", r"\q"),
            ]
            if opening != closing:
                bodies.append(("nested", opening + "a" + closing))
            for suffix, content in bodies:
                add("percent_" + style + "_" + label + "_" + suffix,
                    "%" + style + opening + content + closing)
    for name, body in [
        ("backslash", r"\\"), ("apostrophe", r"\'"), ("double_quote", r'\"'),
        ("hex_literal", r"\x43"), ("unicode_literal", r"\u{e9}"),
        ("meta_literal", r"\M-C"), ("unknown_literal", r"\q"),
        ("raw_unicode", "é"),
    ]:
        add("single_" + name, "'" + body + "'")
    for name, literal in [
        ("dynamic_string", '"#{1}"'), ("dynamic_percent", '%Q{a#{1}}'),
        ("unclosed_double", '"abc'), ("unclosed_single", "'abc"),
        ("unclosed_percent", "%q[abc"), ("empty_character", "?"),
    ]:
        add("classification_" + name, literal)
    assert len({row["id"] for row in rows}) == len(rows)
    return rows


MRI_PREFIX = r'''
puts ["VERSION", RUBY_ENGINE, RUBY_VERSION, RUBY_PATCHLEVEL.to_s, RUBY_REVISION.to_s, RUBY_DESCRIPTION].join("\t")
'''
MRI_SUFFIX = r'''
records.each do |id, source|
  ast_kind = nil
  begin
    tree = RubyVM::AbstractSyntaxTree.parse(source)
    body = tree.children[2]
    ast_kind = body && body.type.to_s
    if ast_kind != "STR"
      puts [id, "non_leaf", ast_kind || "", "", "", ""].join("\t")
      next
    end
    value = eval(source, TOPLEVEL_BINDING, "(literal)", 1)
    raise TypeError, "literal did not produce a String" unless value.is_a?(String)
    source_lines = source.lines
    first_byte = source_lines.take(body.first_lineno - 1).sum(&:bytesize) + body.first_column
    last_byte = source_lines.take(body.last_lineno - 1).sum(&:bytesize) + body.last_column
    raw = source.byteslice(first_byte...last_byte)
    puts [id, "ok", ast_kind, value.encoding.name, value.bytes.map { |b| "%02x" % b }.join, "",
          raw.bytes.map { |b| "%02x" % b }.join, first_byte, last_byte,
          body.first_lineno, body.first_column, body.last_lineno, body.last_column].join("\t")
  rescue SyntaxError => error
    puts [id, "syntax_error", ast_kind || "", "", "", error.class.name].join("\t")
  rescue StandardError => error
    puts [id, "runtime_exception", ast_kind || "", "", "", error.class.name].join("\t")
  end
end
'''
PARSER_SCRIPT = r'''
require "json"
require "parser/ruby27"
inputs = JSON.parse(STDIN.read)
records = inputs.map do |entry|
  diagnostics = []
  ast_kind = nil
  result = begin
    parser = Parser::Ruby27.new
    parser.diagnostics.all_errors_are_fatal = true
    parser.diagnostics.consumer = ->(diagnostic) do
      diagnostics << {"level" => diagnostic.level.to_s, "reason" => diagnostic.reason.to_s}
    end
    buffer = Parser::Source::Buffer.new("(literal)", 1)
    buffer.source = entry.fetch("source")
    ast = parser.parse(buffer)
    ast_kind = ast&.type&.to_s
    if ast_kind != "str"
      {"status" => "non_leaf", "ast_kind" => ast_kind}
    else
      value = ast.children.first
      raise TypeError, "literal did not produce a String" unless value.is_a?(String)
      expression = ast.loc.expression
      {"status" => "ok", "ast_kind" => ast_kind, "encoding" => value.encoding.name,
       "bytes_hex" => value.bytes.map { |b| "%02x" % b }.join,
       "literal_source_hex" => expression.source.bytes.map { |b| "%02x" % b }.join,
       "literal_source_range" => {"unit" => "parser_character", "begin_pos" => expression.begin_pos,
         "end_pos" => expression.end_pos, "first_line" => expression.line,
         "first_column" => expression.column, "last_line" => expression.last_line,
         "last_column" => expression.last_column}}
    end
  rescue Parser::SyntaxError => error
    {"status" => "syntax_error", "ast_kind" => ast_kind, "exception" => error.class.name}
  rescue StandardError => error
    {"status" => "runtime_exception", "ast_kind" => ast_kind, "exception" => error.class.name}
  end
  result["diagnostics"] = diagnostics unless diagnostics.empty?
  {"id" => entry.fetch("id"), "value" => result}
end
puts JSON.generate({
  "runtime" => {"engine" => RUBY_ENGINE, "version" => RUBY_VERSION, "patchlevel" => RUBY_PATCHLEVEL, "revision" => RUBY_REVISION.to_s, "description" => RUBY_DESCRIPTION},
  "parser" => {"version" => Parser::VERSION, "class" => "Parser::Ruby27", "target_ruby" => "2.7"},
  "records" => records
})
'''


def main():
    arguments = argparse.ArgumentParser(description=__doc__)
    arguments.add_argument("--ruby27", required=True, type=Path)
    arguments.add_argument("--parser-ruby", default="ruby")
    arguments.add_argument("--output", required=True, type=Path)
    args = arguments.parse_args()
    rows = inputs()
    with tempfile.TemporaryDirectory(prefix="literal-reference-") as directory:
        directory = Path(directory)
        mri = MRI_PREFIX + "\nrecords = [\n" + ",\n".join(
            "[" + ruby_quote(row["id"]) + ", " + ruby_quote(row["source"]) + "]" for row in rows
        ) + "\n]\n" + MRI_SUFFIX
        (directory / "mri.rb").write_text(mri, encoding="utf-8", newline="")
        (directory / "parser.rb").write_text(PARSER_SCRIPT, encoding="utf-8", newline="")
        physical = subprocess.run([str(args.ruby27), "-W0", str(directory / "mri.rb")],
                                  capture_output=True, text=True, timeout=55, check=True)
        if physical.stderr:
            raise RuntimeError("unexpected MRI recorder stderr: " + physical.stderr[:1000])
        upstream = subprocess.run([args.parser_ruby, "-W0", str(directory / "parser.rb")],
                                  input=json.dumps(rows, ensure_ascii=False), capture_output=True,
                                  text=True, timeout=55, check=True)
        if upstream.stderr:
            raise RuntimeError("unexpected Parser recorder stderr: " + upstream.stderr[:1000])
    lines = physical.stdout.splitlines()
    version = lines.pop(0).split("\t")
    assert version[0] == "VERSION" and version[2] == "2.7.8", version
    assert len(lines) == len(rows), (len(lines), len(rows))
    physical_values = {}
    for line in lines:
        fields = line.split("\t")
        case_id, status, ast_kind, encoding, encoded, exception = fields[:6]
        value = {"status": status, "ast_kind": ast_kind or None}
        if status == "ok":
            assert len(fields) == 13, fields
            raw, first_byte, last_byte, first_line, first_column, last_line, last_column = fields[6:]
            value.update(encoding=encoding, bytes_hex=encoded, literal_source_hex=raw,
                         literal_source_range={"unit": "byte", "begin_pos": int(first_byte),
                             "end_pos": int(last_byte), "first_line": int(first_line),
                             "first_column": int(first_column), "last_line": int(last_line),
                             "last_column": int(last_column)})
        elif exception:
            value["exception"] = exception
        physical_values[case_id] = value
    parsed = json.loads(upstream.stdout)
    assert parsed["parser"]["version"] == "3.3.12.0", parsed["parser"]
    upstream_values = {row["id"]: row["value"] for row in parsed["records"]}
    assert set(physical_values) == set(upstream_values) == {row["id"] for row in rows}
    recorded = []
    for row in rows:
        mri, parser = physical_values[row["id"]], upstream_values[row["id"]]
        statuses = {mri["status"], parser["status"]}
        classification = ("literal_values" if statuses == {"ok"} else
                          "reference_error" if statuses & {"syntax_error", "runtime_exception"} else "non_leaf")
        recorded.append({**row, "classification": classification, "mri27": mri, "parser27": parser})
    output = {
        "provenance": "Synthetic string and character literal sources; actual MRI 2.7.8 and Parser::Ruby27 values, one isolated recorder process per runtime. No dstr evaluation is used as a literal expectation.",
        "mri_runtime": {"engine": version[1], "version": version[2], "patchlevel": int(version[3]), "revision": version[4], "description": version[5]},
        "parser_runtime": parsed["runtime"],
        "parser": parsed["parser"],
        "summary": {
            "cases": len(recorded),
            "classifications": dict(collections.Counter(row["classification"] for row in recorded)),
            "mri_statuses": dict(collections.Counter(row["mri27"]["status"] for row in recorded)),
            "parser_statuses": dict(collections.Counter(row["parser27"]["status"] for row in recorded)),
        },
        "cases": recorded,
    }
    encoded = (json.dumps(output, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    assert b"/home/" not in encoded and b"C:\\" not in encoded
    args.output.write_bytes(encoded)
    print(json.dumps({**output["summary"], "sha256": hashlib.sha256(encoded).hexdigest()}, ensure_ascii=False))


if __name__ == "__main__":
    main()