#!/usr/bin/env python3
"""RuboCop 1.89.0 の実 CLI から静的 regexp の診断・終了値・補正バイトを記録する。"""
import argparse
import concurrent.futures
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINDING_COPS = "Layout/SpaceAroundOperators,Lint/AmbiguousOperator"
VERSIONS = [("2.7", "default"), ("3.2", "parser_whitequark"), ("3.3", "default"), ("3.3", "parser_whitequark"), ("3.4", "parser_whitequark"), ("3.4", "parser_prism")]
PATTERNS = {
    "plain": r"/(?<y>.)/",
    "literal": r"/#{'a'}(?<y>.)/",
    "empty": r"/(?<y>.)#{}/",
    "statements": r"/#{'a'; 'b'}(?<y>.)/",
    "leading_empty": r"/#{; 'a';}(?<y>.)/",
    "leading_empty_paren": r"/#{(; 'a')}(?<y>.)/",
    "middle_empty_paren": r"/(?<y>.)#{'a'; (); 'b'}/",
    "first_empty_paren": r"/(?<y>.)#{(); 'b'}/",
    "middle_semicolon": r"/(?<y>.)#{'a'; ; 'b'}/",
    "percent_open_angle": r"/#{%q<(?\<y\>.)>}/",
    "percent_open_paren": r"/#{%q(\(?<y>.\))}/",
    "percent_open_bracket": r"/#{%q[\[(?<y>.)\]]}/",
    "percent_open_brace": r"/#{%q{\{(?<y>.)\}}}/",
    "empty_string": r"/#{''}(?<y>.)/",
    "empty_string_paren": r"/#{('')}(?<y>.)/",
    "discarded_capture": r"/#{'(?<y>.)'; 'a'}/",
    "discarded_extended": r"/#{'(?x)'; '#(?<y>.)'}/",
    "character": r"/#{?a}(?<y>.)/",
    "character_raw_control_x": r"/(?<y>.)#{?\C-x}/",
    "character_raw_shortcontrol_x": r"/(?<y>.)#{?\cx}/",
    "character_control": r"/#{?\C-\x43}(?<y>.)/",
    "character_short_control": r"/#{?\c\x43}(?<y>.)/",
    "character_control_parentheses": r"/#{((?\C-\x43))}(?<y>.)/",
    "character_control_statements": r"/#{?\C-\x43; 'a'}(?<y>.)/",
    "character_control_dynamic": r"/#{?\C-\x43; value}(?<y>.)/",
    "character_control_arithmetic": r"/#{?\C-\x43 + ''}(?<y>.)/",
    "parentheses": r"/#{(('a'))}(?<y>.)/",
    "chained": r"/#{'a' 'b'}(?<y>.)/",
    "nested": r'''/#{"#{'a'}"}(?<y>.)/''',
    "dynamic": r"/#{value}(?<y>.)/",
    "byte_escapes": r'/#{"\x28\x3f\x3cy\x3e."})/',
    "split_group": r"/#{'(?<'}#{'y'}#{'>.'})/",
    "escaped_backslash": r'''/#{'\\\\'}(?<y>.)/''',
    "extended_comment": "/#{\"# (?<hidden>a)\\n\"}(?<y>.)/x",
    "extended_class": r"/#{'[(?<hidden>)]'}(?<y>.)/x",
    "inline_extended": "/#{\"(?x:# (?<hidden>a)\\n)\"}(?<y>.)/",
    "percent_single": r"%r{#{%q[(?<y>.)]}}",
    "multiline": "/#{\"a\nb\"}(?<y>.)/",
    "trailing_newline": "/#{\"a\n\"}(?<y>.)/",
    "continued_double": "/#{\"a\\\nb\"}(?<y>.)/",
    "continued_single": "/#{'a\\\nb'}(?<y>.)/",
}


def cases():
    for name, atom in [("raw_meta_x", r"?\M-x"), ("raw_meta_u", r"?\M-u")]:
        yield name, "# encoding: ASCII-8BIT\n" + f"/(?<y>.)#{{{atom}}}/ =~ text\ny +1\n", BINDING_COPS, ""
    yield "nul_bound", "/#{'a'}(?<y>.)/ =~ text\ny /1\0+2/\n", "Lint/Syntax", ""
    yield "verification_bound", "def operator(text)\n  /#{'a'}(?<y>.)/ =~ text\n  (y +1)\nend\n", "Style/RedundantParentheses", ""
    for name, pattern in PATTERNS.items():
        yield name, f"def operator(text)\n  {pattern} =~ text\n  y +1\nend\n", BINDING_COPS, ""
    for name, pattern in [("literal", r"/#{'a'}(?<y>.)/"), ("plain", r"/(?<y>.)/"), ("dynamic", r"/#{x}(?<y>.)/")]:
        for used in [False, True]:
            tail = "  y\n" if used else ""
            yield f"capture_usage_{name}_{used}", f"def operator(text)\n  {pattern} =~ text\n{tail}end\n", "Lint/UselessAssignment", ""
        yield f"capture_metrics_{name}", f"def operator(text)\n  {pattern} =~ text\n  y +1\nend\n", "Metrics/AbcSize,Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity", "Metrics/AbcSize:\n  Max: 0\nMetrics/CyclomaticComplexity:\n  Max: 0\nMetrics/PerceivedComplexity:\n  Max: 0\n"
    yield "capture_naming", "def operator(text)\n  /#{'a'}(?<_y>.)/ =~ text\n  _y\nend\n", "Lint/UnderscorePrefixedVariableName", ""
    for name, content, quote in [("heredoc_one", "  a\n", ""), ("heredoc_two", "  a\n  b\n", ""), ("heredoc_empty", "", ""), ("heredoc_single", "  a\n", "'"), ("heredoc_continued", "  a\\\n  b\n", "")]:
        yield name, f"def operator(text)\n  /#{{<<~{quote}X{quote}}}(?<y>.)/ =~ text\n{content}X\n  y +1\nend\n", BINDING_COPS, ""
    for delimiter in ["slash", "percent"]:
        for indent in ["    ", "\t"]:
            pattern = "/#{<<~X}/" if delimiter == "slash" else "%r{#{<<~X}}"
            name = f"heredoc_split_{delimiter}_{'tab' if indent == chr(9) else 'spaces'}"
            yield name, f"def operator(text)\n  {pattern} =~ text\n{indent}(?<\\\n{indent}y>.)\n  X\n  y +1\nend\n", BINDING_COPS, ""
    for name, pattern in [("plain", "/a/"), ("static", r"/#{'a'}/"), ("dynamic", r"/a#{x}/")]:
        for operator in ["+", "=="]:
            yield f"dispatch_{name}_{operator}", f"def operator(a, b)\n  a += {pattern} {operator} b\nend\n", "Metrics/AbcSize,Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity", "Metrics/AbcSize:\n  Max: 0\nMetrics/CyclomaticComplexity:\n  Max: 0\nMetrics/PerceivedComplexity:\n  Max: 0\n"
        yield f"variables_{name}", f"def operator(text)\n  {pattern} =~ text\n  y +1\nend\n", "Lint/UselessAssignment", ""
        yield f"operators_{name}", f"x = 1\ny = {pattern}=~x\n", "Layout/SpaceAroundOperators", ""
        yield f"inverse_{name}", f"x = 1\ny = !({pattern} =~ x)\n", "Style/InverseMethods", ""
        yield f"select_{name}", f"x = 1\nvalues.select {{ |value| {pattern} =~ value }}\n", "Style/SelectByRegexp", ""
        yield f"range_{name}", f"x = 1\n{pattern} =~ x\np $1\n", "Lint/OutOfRangeRegexpRef", ""
        yield f"modifier_{name}", f"x = 1\nif {pattern} =~ x\n  use x\nend\n", "Style/IfUnlessModifier", ""
        yield f"ambiguous_regexp_{name}", f"x = 1\np {pattern} =~ x\n", "Lint/AmbiguousRegexpLiteral", ""
        yield f"metrics_{name}", f"def operator(x)\n  {pattern} =~ x\nend\n", "Metrics/AbcSize,Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity", "Metrics/AbcSize:\n  Max: 0\nMetrics/CyclomaticComplexity:\n  Max: 0\nMetrics/PerceivedComplexity:\n  Max: 0\n"
    for name, literal in [("utf8_bytes", r'"(?<\xc3\xa9>x)"'), ("meta_hex", r'"(?<\M-\x43\M-\x29>x)"'), ("meta_octal", r'"(?<\M-\103\M-\051>x)"'), ("unicode_fixed", r'"(?<\u00e9>x)"'), ("unicode_braced", r'"(?<\u{e9}>x)"'), ("raw", '"(?<é>x)"')]:
        for binary in [False, True]:
            header = "# encoding: ASCII-8BIT\n" if binary else "# encoding: UTF-8\n"
            yield f"encoding_{name}_{binary}", header + f"/#{{{literal}}}/ =~ text\né +1\n", BINDING_COPS, ""



def record(item):
    group, (version, engine), ending, mode = item
    _, _, only, settings = group[0]
    # 無効 UTF-8 regexp の compile 診断は #30 の regexp_validation gate へ移す。
    # 本家の fatal を、現在の候補の沈黙や既知差分に書き換えてはならない。
    whitequark = engine == "parser_whitequark" or engine == "default" and tuple(map(int, version.split("."))) < (3, 3)
    delegated = [case[0] for case in group if whitequark and case[0] in {"encoding_meta_hex_False", "encoding_meta_octal_False"}]
    group = [case for case in group if case[0] not in delegated]
    config = f"AllCops:\n  NewCops: enable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\n" + settings
    with tempfile.TemporaryDirectory(prefix="sonicop-regexp-reference-") as directory:
        path = Path(directory)
        sources = {}
        for index, (name, source, _, _) in enumerate(group):
            filename = f"case{index:03}.rb"
            source = source.replace("\n", "\r\n" if ending == "crlf" else "\n")
            (path / filename).write_bytes(source.encode())
            sources[filename] = (name, source)
        (path / ".rubocop.yml").write_text(config, encoding="utf-8", newline="")
        args = ["rubocop", "_1.89.0_", "--cache", "false", "--raise-cop-error", "--format", "json", "--only", only]
        if mode != "inspect":
            args.append(mode)
        completed = subprocess.run(args + list(sources), cwd=path, capture_output=True, timeout=120)
        try:
            output = json.loads(completed.stdout)
        except ValueError as error:
            raise ValueError(f'{only}/{version}/{engine}/{ending}/{mode}: {completed.returncode}: {completed.stderr.decode()[:3000]} stdout={completed.stdout[:1000]!r}') from error
        for field in ["inspected_file_count", "target_file_count"]:
            if output["summary"][field] != len(sources):
                raise ValueError(f"incomplete reference: {field}: {output['summary']}")
        if len(output["files"]) != len(sources):
            raise ValueError("reference returned a different file count")
        reports = {report["path"]: report["offenses"] for report in output["files"]}
        records = []
        for filename, (name, source) in sources.items():
            if filename not in reports:
                raise ValueError(f"{name}/{version}/{engine}/{ending}/{mode}: reference did not inspect file: {completed.stderr.decode()[:5000]}")
            offenses = reports[filename]
            after = (path / filename).read_bytes().decode()
            # 本家の CRLF→LF 書き戻しだけは既知の差。診断の座標はそのまま記録する。
            if after != source and ending == "crlf" and name != "truncated_hex_escape":
                after = after.replace("\r\n", "\n").replace("\n", "\r\n")
            # 例外終了を正常な lint と混同しない。
            if completed.returncode not in [0, 1]:
                raise ValueError(f"reference failed: {only}/{version}/{engine}/{ending}/{mode}: {completed.returncode}: {completed.stderr.decode()[:5000]}")
            records.append({"id": name, "path": filename, "source": source, "offenses": offenses, "after": after})
    return {"id": f"{only}/{version}/{engine}/{ending}/{mode}", "target": f"{version}/{engine}", "ending": ending, "config": config, "only": only, "mode": mode, "exit": completed.returncode, "files": records, "delegated_to_regexp_validation": delegated}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--character-boundary-only", action="store_true")
    parser.add_argument("--workers", type=int, default=4)
    arguments = parser.parse_args()
    if arguments.output is None:
        arguments.output = ROOT / "tests/fixtures" / ("regexp_character_boundary.json" if arguments.character_boundary_only else "regexp_semantics.json")
    groups = {}
    for case in cases():
        groups.setdefault((case[2], case[3]), []).append(case)
    matrix = itertools.product(groups.values(), VERSIONS, ["lf", "crlf"], ["inspect", "-a", "-A"])
    if arguments.character_boundary_only:
        # MRI で無効な character の既存差。通常一致・72 条件の安全 binding 差とは別記録。
        source = "def y(value)\n  value\nend\n/#{?\\C-\\x}(?<y>.)/ =~ 'a'\np y +1\n"
        group = [("truncated_hex_escape", source, "Layout/SpaceAroundOperators,Lint/Syntax", "")]
        matrix = itertools.product([group], [("2.7", "default")], ["lf", "crlf"], ["-a", "-A"])
    with concurrent.futures.ThreadPoolExecutor(max_workers=arguments.workers) as pool:
        records = list(pool.map(record, matrix))
    arguments.output.write_text("[\n" + ",\n".join(json.dumps(record, ensure_ascii=False, separators=(",", ":")) for record in records) + "\n]\n", encoding="utf-8", newline="")
    print(f"{sum(len(record['files']) for record in records)} file conditions / {len(records)} CLI invocations recorded from RuboCop 1.89.0; {sum(len(record['delegated_to_regexp_validation']) for record in records)} invalid-regexp conditions delegated to #30")


if __name__ == "__main__":
    main()