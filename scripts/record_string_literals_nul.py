#!/usr/bin/env python3
"""RuboCop 1.89.0 の NUL 文字列の実診断・補正 bytes・MRI の値を記録する。"""
import argparse
import concurrent.futures
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = [("2.7", "default"), ("2.7", "parser_whitequark"), ("3.3", "default"), ("3.3", "parser_whitequark"), ("3.4", "default"), ("3.4", "parser_whitequark"), ("3.4", "parser_prism")]
CASES = {
    "plain_double": 'a="xy"\n',
    "plain_single": "a='xy'\n",
    "raw_middle": 'a="x\0y"\n',
    "raw_leading": 'a="\0y"\n',
    "raw_terminal": 'a="x\0"\n',
    "raw_only": 'a="\0"\n',
    "raw_consecutive": 'a="\0\0"\n',
    "raw_single_quoted": "a='x\0y'\n",
    "single_terminal": "a='x\0'\n",
    "escaped_octal": 'a="x\\0y"\n',
    "escaped_hex": 'a="x\\x00y"\n',
    "escaped_unicode": 'a="x\\u0000y"\n',
    "escaped_backslash": 'a="x\\\\0y"\n',
    "escaped_quote": 'a="x\\"\0y"\n',
    "apostrophe": 'a="x\'\0y"\n',
    "interpolated": 'a="x\0#{1}y"\n',
    "interpolated_literal": 'a="#{"x\0y"}"\n',
    "chained_double": 'a="x\0y" "z"\n',
    "chained_single": "a='x\0y' 'z'\n",
    "chained_mixed": 'a="x\0y" \'z\'\n',
    "comment_nul": '# x\0y\na="xy"\n',
    "trailing_comment": 'a="xy" # x\0y\n',
    "heredoc": 'a=<<~TXT\nx\0y\nTXT\n',
    "nul_then_line_break": 'a="x\0\n"\n',
    "soh": 'a="x\x01y"\n',
    "tab": 'a="x\ty"\n',
    "escape_control": 'a="x\x1by"\n',
    "percent_single": 'a=%q(x\0y)\n',
    "utf8": 'a="é\0あ"\n',
}
RUNTIME = r'''
puts "VERSION\t#{RUBY_VERSION}\t#{RUBY_DESCRIPTION}"
ARGV.each do |path|
  source = File.binread(path).force_encoding('UTF-8')
  value = Object.new.instance_eval { eval(source, binding, path) }
  raise "not a String: #{path}" unless value.is_a?(String)
  puts [File.basename(path), value.encoding.name, value.b.unpack1('H*')].join("\t")
end
'''


def values(ruby, folder, names):
    result = subprocess.run([str(ruby), '-W0', '-e', RUNTIME, *names], cwd=folder, capture_output=True, timeout=30, check=True)
    if result.stderr:
        raise RuntimeError(result.stderr.decode()[:2000])
    lines = result.stdout.decode().splitlines()
    version = lines.pop(0).split('\t')
    if version[1] != '2.7.8':
        raise ValueError(version)
    records = {}
    for line in lines:
        name, encoding, encoded = line.split('\t')
        records[name] = {'encoding': encoding, 'hex': encoded}
    if set(records) != set(names):
        raise ValueError('incomplete runtime values')
    return records


def record(item):
    version, engine, ending, mode, style, consistent, kind, ruby = item
    source_cases = dict(CASES)
    if kind == 'binary_inspection':
        source_cases = {name: '# encoding: ASCII-8BIT\n' + source for name, source in CASES.items() if name in {'raw_middle', 'raw_terminal', 'raw_only', 'escaped_hex', 'soh'}}
    config = f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\nStyle/StringLiterals:\n  Enabled: true\n  EnforcedStyle: {style}\n  ConsistentQuotesInMultiline: {str(consistent).lower()}\n'
    with tempfile.TemporaryDirectory(prefix='string-literals-nul-reference-') as directory:
        folder = Path(directory)
        (folder / '.rubocop.yml').write_text(config, encoding='utf-8', newline='')
        originals = {}
        for index, (name, text) in enumerate(source_cases.items()):
            path = f'case{index:03}.rb'
            raw = text.replace('\n', '\r\n' if ending == 'crlf' else '\n').encode()
            (folder / path).write_bytes(raw)
            originals[path] = (name, raw)
        before = values(ruby, folder, originals)
        args = ['rubocop', '_1.89.0_', '--raise-cop-error', '--cache', 'false', '--format', 'json', '--only', 'Style/StringLiterals']
        if mode != 'inspect':
            args.append(mode)
        result = subprocess.run(args + list(originals), cwd=folder, capture_output=True, timeout=90)
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr.decode()[:3000])
        report = json.loads(result.stdout)
        if report['summary']['target_file_count'] != len(originals) or report['summary']['inspected_file_count'] != len(originals):
            raise ValueError('incomplete reference inspection')
        files = {file['path']: file['offenses'] for file in report['files']}
        if set(files) != set(originals):
            raise ValueError('different reference target set')
        after_values = values(ruby, folder, originals)
        files_record = []
        for path, (name, raw) in originals.items():
            actual_after = (folder / path).read_bytes()
            after = actual_after
            # 既知の CRLF 保持差だけを候補の形式へ戻し、実本家 bytes も別に保存する。
            if ending == 'crlf' and after != raw:
                after = after.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
            files_record.append({'id': name, 'path': path, 'source_hex': raw.hex(), 'offenses': files[path], 'after_hex': after.hex(), 'raw_reference_after_hex': actual_after.hex(), 'before_value': before[path], 'after_value': after_values[path]})
    return {'target': f'{version}/{engine}', 'ending': ending, 'mode': mode, 'kind': kind, 'config': config, 'exit': result.returncode, 'summary': report['summary'], 'files': files_record}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ruby27', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'tests/fixtures/string_literals_nul.json')
    parser.add_argument('--workers', type=int, default=3)
    args = parser.parse_args()
    matrix = [(*target, ending, mode, style, consistent, 'regular', args.ruby27) for target, ending, mode, style, consistent in itertools.product(TARGETS, ['lf', 'crlf'], ['inspect', '-a', '-A'], ['single_quotes', 'double_quotes'], [False, True])]
    # ASCII-8BIT の inspect_body 書戻し差は別の既存不具合。ここでは raw NUL の AST 分類を診断だけで比較する。
    matrix += [(*target, ending, 'inspect', style, consistent, 'binary_inspection', args.ruby27) for target, ending, style, consistent in itertools.product([('2.7', 'parser_whitequark'), ('3.4', 'parser_prism')], ['lf', 'crlf'], ['single_quotes', 'double_quotes'], [False, True])]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        records = list(pool.map(record, matrix))
    args.output.write_text('[\n' + ',\n'.join(json.dumps(row, ensure_ascii=False, separators=(',', ':')) for row in records) + '\n]\n', encoding='utf-8', newline='')
    # 既存 CRLF 復号差は誤った候補 bytes を期待にせず、本家の生記録を別に残す。
    pending = []
    for index, row in enumerate(records):
        prism = row['target'] in {'3.3/default', '3.4/default', '3.4/parser_prism'}
        if prism and row['ending'] == 'crlf' and row['mode'] != 'inspect' and 'EnforcedStyle: single_quotes\n' in row['config']:
            for case in row['files']:
                if case['id'] == 'nul_then_line_break':
                    pending.append({'group': index, 'issue': 54, 'target': row['target'], 'ending': row['ending'], 'mode': row['mode'], 'config': row['config'], 'reason': '既存の物理 CRLF 復号による値の変更。補正 bytes は Issue #54 で検証する。', **case})
    if len(pending) != 12:
        raise ValueError('incomplete pending CRLF conditions')
    args.output.with_name(args.output.stem + '_pending.json').write_text(json.dumps(pending, ensure_ascii=False, indent=2) + '\n', encoding='utf-8', newline='')
    print(f'{len(records)} CLI invocations; {sum(len(row["files"]) for row in records)} completed file conditions')


if __name__ == '__main__':
    main()
