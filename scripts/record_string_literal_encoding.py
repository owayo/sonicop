#!/usr/bin/env python3
"""実 RuboCop 1.89.0 と MRI 2.7.8 で文字列補正の符号化別出力を記録する。"""
import argparse
import concurrent.futures
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

from record_string_literals_nul import values

ROOT = Path(__file__).resolve().parents[1]
HEADERS = {
    'default': b'',
    'utf8': b'# encoding: UTF-8\n',
    'binary': b'# encoding: ASCII-8BIT\n',
    'binary_alias': b'# coding: BINARY\n',
    'binary_lower': b'# coding: ascii-8bit\n',
    'seven_bit': b'# encoding: US-ASCII\n',
    'second_comment': b'# ordinary comment\n# encoding: BINARY\n',
}
CONTENTS = {
    'plain': b'xy',
    'soh': b'x\x01y',
    'nul_terminal': b'x\0',
    'tab': b'x\ty',
    'bell': b'x\ay',
    'backspace': b'x\x08y',
    'vertical_tab': b'x\x0by',
    'form_feed': b'x\x0cy',
    'carriage_return': b'x\ry',
    'escape': b'x\x1by',
    'unit_separator': b'x\x1fy',
    'delete': b'x\x7fy',
    'octal_escape': br'x\1y',
    'hex_escape': br'x\x01y',
    'unicode_escape': br'x\u0001y',
    'unicode_nonascii_escape': br'x\u3042y',
    'unicode_braced_escape': br'x\u{3042}y',
    'backslash': br'x\\y',
    'double_quote': b'x"y',
    'single_quote': b"x'y",
    'interpolation_text': b'x#{1}',
    'encoding_text': b'coding: BINARY x\x01y',
    'matching_quote': b'xy',
}


def sources(cop, style, ending):
    for header_name, header in HEADERS.items():
        contents = dict(CONTENTS)
        if header_name != 'seven_bit':
            contents['utf8'] = 'éあ'.encode()
        if header_name in {'binary', 'binary_alias', 'binary_lower'}:
            contents.update({f'high_byte_{point:02x}': b'x' + bytes([point]) + b'y' for point in [0x80, 0xa0, 0xe9, 0xff]})
            contents['mixed_binary'] = b'\xe9' + 'あ'.encode() + b'\xff'
            contents['mixed_binary_control'] = b'\1' + 'é'.encode() + b'\xe9'
        for name, content in contents.items():
            quote = b'"' if style == 'single_quotes' else b"'"
            if name == 'matching_quote':
                quote = b"'" if style == 'single_quotes' else b'"'
            # 引用符の制約だけをエスケープし、実制御文字と元の escape を混同しない。
            literal = quote + content.replace(quote, b'\\' + quote) + quote
            expression = b'"#{' + literal + b'}"' if cop.endswith('InInterpolation') else literal
            raw = header + b'a=' + expression + b'\n'
            if ending == 'crlf':
                raw = raw.replace(b'\n', b'\r\n')
            yield header_name + '/' + name, raw


def record(item):
    (version, engine), cop, style, ending, mode, ruby = item
    config = f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\n{cop}:\n  Enabled: true\n  EnforcedStyle: {style}\n'
    with tempfile.TemporaryDirectory(prefix='string-encoding-reference-') as tmp:
        folder = Path(tmp)
        (folder / '.rubocop.yml').write_text(config, encoding='utf-8', newline='')
        originals = {}
        for index, (name, raw) in enumerate(sources(cop, style, ending)):
            path = f'case{index:03}.rb'
            originals[path] = (name, raw)
            (folder / path).write_bytes(raw)
        before = values(ruby, folder, list(originals))
        command = ['rubocop', '_1.89.0_', '--raise-cop-error', '--cache', 'false', '--format', 'json', '--only', cop]
        if mode != 'inspect':
            command.append(mode)
        result = subprocess.run(command + list(originals), cwd=folder, capture_output=True, timeout=90)
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr.decode()[:3000])
        report = json.loads(result.stdout)
        if report['summary']['inspected_file_count'] != len(originals) or report['summary']['target_file_count'] != len(originals):
            raise ValueError('incomplete reference inspection')
        files = {entry['path']: entry['offenses'] for entry in report['files']}
        if len(report['files']) != len(originals) or set(files) != set(originals):
            raise ValueError('different reference target set')
        after = values(ruby, folder, list(originals))
        records = []
        for path, (name, raw) in originals.items():
            if before[path] != after[path]:
                raise ValueError(f'reference changed runtime value: {name}: {before[path]} / {after[path]}')
            actual = (folder / path).read_bytes()
            normalized = actual
            # 許容する CRLF 差は末尾の保存形式だけ。文字列内の物理改行はこの採取対象に含めない。
            if ending == 'crlf' and normalized != raw:
                normalized = normalized.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
            records.append({'id': name, 'path': path, 'source_hex': raw.hex(), 'offenses': files[path], 'after_hex': normalized.hex(), 'raw_reference_after_hex': actual.hex(), 'before_value': before[path], 'after_value': after[path]})
    return {'target': f'{version}/{engine}', 'cop': cop, 'style': style, 'ending': ending, 'mode': mode, 'config': config, 'exit': result.returncode, 'summary': report['summary'], 'files': records}


def main():
    global CONTENTS, HEADERS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ruby27', type=Path, required=True)
    parser.add_argument('--workers', type=int, default=3)
    parser.add_argument('--multiline', action='store_true', help='Prism の物理 LF 部品結合による値の符号化を記録する')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.multiline:
        HEADERS = {'binary': HEADERS['binary']}
        CONTENTS = {
            'plain': b'xy',
            'soh_then_line_break': b'x\1\n',
            'nul_then_line_break': b'x\0\n',
            'invalid_byte_then_line_break': b'x\xe9\n',
            'utf8_then_line_break': 'éあ\n'.encode(),
            'byte_escape_then_line_break': br'x\xE9' + b'\n',
            'unicode_escape_then_line_break': br'x\u00e9' + b'\n',
            'continued_line': b'x\1\\\ny',
            'escaped_line_break': br'x\n',
            'soh_without_line_break': b'x\1y',
        }
    if args.output is None:
        name = 'string_literal_multiline_encoding.json' if args.multiline else 'string_literal_encoding.json'
        args.output = ROOT / 'tests/fixtures' / name
    # 本家の to_string_literal は既定外部符号化にも依存する。測定条件が違えば記録を混ぜない。
    runtime = subprocess.run(['ruby', '-rjson', '-e', 'puts JSON.generate({ruby: RUBY_DESCRIPTION, default_external: Encoding.default_external.name})'], capture_output=True, check=True)
    reference_runtime = json.loads(runtime.stdout)
    if reference_runtime['default_external'] != 'UTF-8':
        raise ValueError(reference_runtime)
    matrix = [(*item, args.ruby27) for item in itertools.product([('2.7', 'parser_whitequark'), ('3.4', 'parser_prism')], ['Style/StringLiterals', 'Style/StringLiteralsInInterpolation'], ['single_quotes', 'double_quotes'], ['lf'] if args.multiline else ['lf', 'crlf'], ['inspect', '-a', '-A'])]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        records = list(pool.map(record, matrix))
    args.output.write_text('[\n' + ',\n'.join(json.dumps(row, ensure_ascii=False, separators=(',', ':')) for row in records) + '\n]\n', encoding='utf-8', newline='')
    print(f'{len(records)} CLI invocations / {sum(len(row["files"]) for row in records)} file conditions')


if __name__ == '__main__':
    main()