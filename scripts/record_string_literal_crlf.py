#!/usr/bin/env python3
"""実 RuboCop と MRI 2.7.8 から、物理 CRLF を含む文字列の補正を記録する。"""
import argparse
import concurrent.futures
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

from record_string_literals_nul import values

ROOT = Path(__file__).resolve().parents[1]
HEADERS = {'utf8': b'', 'binary': b'# encoding: BINARY\n', 'seven_bit': b'# encoding: US-ASCII\n'}
CONTENTS = {
    'plain': b'xy',
    'terminal_line_break': b'x\n',
    'only_line_break': b'\n',
    'leading_line_break': b'\nx',
    'middle_line_break': b'x\ny',
    'two_line_breaks': b'x\n\n',
    'raw_carriage_return': b'x\ry',
    'escaped_line_break': br'x\n',
    'escaped_carriage_return': br'x\r',
    'escaped_crlf': br'x\r\n',
    'escaped_backslash': br'x\\',
    'backslash_then_line_break': b'x\\\n',
    'two_backslashes_then_line_break': b'x\\\\\n',
    'three_backslashes_then_line_break': b'x\\\\\\\n',
    'escaped_quote_then_line_break': b'x"\n',
    'apostrophe_then_line_break': b"x'\n",
    'soh_then_line_break': b'x\x01\n',
    'matching_quote': b'x\n',
}


def sources(cop, style, ending):
    for header_name, header in HEADERS.items():
        contents = dict(CONTENTS)
        if header_name != 'seven_bit':
            contents['utf8_then_line_break'] = 'éあ\n'.encode()
        if header_name == 'binary':
            contents['invalid_byte_then_line_break'] = b'x\xe9\n'
        if cop == 'Style/StringLiterals':
            contents['nul_then_line_break'] = b'x\0\n'
        for name, content in contents.items():
            quote = b'"' if style == 'single_quotes' else b"'"
            if name == 'matching_quote':
                quote = b"'" if style == 'single_quotes' else b'"'
            literal = quote + content.replace(quote, b'\\' + quote) + quote
            expression = b'"#{' + literal + b'}"' if cop.endswith('InInterpolation') else literal
            raw = header + b'a=' + expression + b'\n'
            if ending == 'crlf':
                raw = raw.replace(b'\n', b'\r\n')
            yield header_name + '/' + name, raw


def record(item):
    (version, engine), cop, style, consistent, ending, mode, ruby = item
    config = f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\n{cop}:\n  Enabled: true\n  EnforcedStyle: {style}\n'
    if not cop.endswith('InInterpolation'):
        config += f'  ConsistentQuotesInMultiline: {str(consistent).lower()}\n'
    with tempfile.TemporaryDirectory(prefix='string-crlf-reference-') as tmp:
        folder = Path(tmp)
        (folder / '.rubocop.yml').write_text(config, encoding='utf-8', newline='')
        originals = {}
        for index, (name, raw) in enumerate(sources(cop, style, ending)):
            path = f'case{index:03}.rb'
            originals[path] = (name, raw)
            (folder / path).write_bytes(raw)
        before = values(ruby, folder, originals)
        command = ['rubocop', '_1.89.0_', '--raise-cop-error', '--cache', 'false', '--format', 'json', '--only', cop]
        if mode != 'inspect':
            command.append(mode)
        result = subprocess.run(command + list(originals), cwd=folder, capture_output=True, timeout=90)
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr.decode()[:3000])
        report = json.loads(result.stdout)
        if any(report['summary'][key] != len(originals) for key in ['inspected_file_count', 'target_file_count']):
            raise ValueError('incomplete reference inspection')
        files = {entry['path']: entry['offenses'] for entry in report['files']}
        if len(report['files']) != len(originals) or set(files) != set(originals):
            raise ValueError('different reference target set')
        after = values(ruby, folder, originals)
        records = []
        for path, (name, raw) in originals.items():
            if before[path] != after[path]:
                raise ValueError(f'reference changed runtime value: {name}: {before[path]} / {after[path]}')
            actual = (folder / path).read_bytes()
            normalized = actual
            # 原本の保存形式だけを維持する。実 Rbc の出力 bytes と実 Ruby 値は別欄に残す。
            if ending == 'crlf' and normalized != raw:
                normalized = normalized.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
            records.append({'id': name, 'path': path, 'source_hex': raw.hex(), 'offenses': files[path], 'after_hex': normalized.hex(), 'raw_reference_after_hex': actual.hex(), 'before_value': before[path], 'after_value': after[path]})
    return {'target': f'{version}/{engine}', 'cop': cop, 'style': style, 'consistent': consistent, 'ending': ending, 'mode': mode, 'config': config, 'exit': result.returncode, 'summary': report['summary'], 'files': records}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ruby27', type=Path, required=True)
    parser.add_argument('--workers', type=int, default=3)
    parser.add_argument('--output', type=Path, default=ROOT / 'tests/fixtures/string_literal_crlf.json')
    args = parser.parse_args()
    matrix = [(*item, args.ruby27) for item in itertools.product([('2.7', 'parser_whitequark'), ('3.4', 'parser_prism')], ['Style/StringLiterals', 'Style/StringLiteralsInInterpolation'], ['single_quotes', 'double_quotes'], [False, True], ['lf', 'crlf'], ['inspect', '-a', '-A']) if item[1] == 'Style/StringLiterals' or not item[3]]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        records = list(pool.map(record, matrix))
    args.output.write_text('[\n' + ',\n'.join(json.dumps(row, ensure_ascii=False, separators=(',', ':')) for row in records) + '\n]\n', encoding='utf-8', newline='')
    print(f'{len(records)} CLI invocations / {sum(len(row["files"]) for row in records)} file conditions')


if __name__ == '__main__':
    main()
