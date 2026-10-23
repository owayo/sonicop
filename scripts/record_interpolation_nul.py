#!/usr/bin/env python3
"""補間内 str の raw NUL 境界を実 RuboCop CLI と MRI 2.7.8 で記録する。"""
import argparse
import concurrent.futures
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

from record_string_literals_nul import values

ROOT = Path(__file__).resolve().parents[1]
CONTENTS = {'plain': b'xy', 'middle': b'x\0y', 'leading': b'\0y', 'terminal': b'x\0', 'only': b'\0', 'consecutive': b'\0\0', 'escaped_octal': br'x\0y', 'escaped_hex': br'x\x00y', 'escaped_unicode': br'x\u0000y', 'escaped_backslash': br'x\\0y', 'soh': b'x\1y', 'quote': b'x"\0y', 'apostrophe': b"x'\0y", 'matching_quote': b'x\0y', 'chained': b'x\0y', 'comment_nul': b'xy'}
COP = 'Style/StringLiteralsInInterpolation'


def record(item):
    (version, engine), style, ending, mode, ruby = item
    config = f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\n{COP}:\n  Enabled: true\n  EnforcedStyle: {style}\n'
    with tempfile.TemporaryDirectory(prefix='interpolation-nul-reference-') as tmp:
        folder = Path(tmp)
        (folder / '.rubocop.yml').write_text(config, encoding='utf-8', newline='')
        originals = {}
        for index, (name, content) in enumerate(CONTENTS.items()):
            quote = b'"' if style == 'single_quotes' else b"'"
            if name == 'matching_quote':
                quote = b"'" if style == 'single_quotes' else b'"'
            literal = quote + content.replace(quote, b'\\' + quote) + quote
            if name == 'chained':
                literal += b" 'z'"
            raw = b'a="#{' + literal + b'}"\n'
            if name == 'comment_nul':
                raw = b'# x\0y\n' + raw
            if ending == 'crlf':
                raw = raw.replace(b'\n', b'\r\n')
            path = f'case{index:03}.rb'
            originals[path] = (name, raw)
            (folder / path).write_bytes(raw)
        before = values(ruby, folder, originals)
        command = ['rubocop', '_1.89.0_', '--raise-cop-error', '--cache', 'false', '--format', 'json', '--only', COP]
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
                raise ValueError(f'reference changed runtime value: {name}')
            actual = (folder / path).read_bytes()
            normalized = actual
            if ending == 'crlf' and normalized != raw:
                normalized = normalized.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
            records.append({'id': name, 'path': path, 'source_hex': raw.hex(), 'offenses': files[path], 'after_hex': normalized.hex(), 'raw_reference_after_hex': actual.hex(), 'before_value': before[path], 'after_value': after[path]})
    return {'target': f'{version}/{engine}', 'cop': COP, 'style': style, 'ending': ending, 'mode': mode, 'config': config, 'exit': result.returncode, 'summary': report['summary'], 'files': records}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ruby27', type=Path, required=True)
    parser.add_argument('--workers', type=int, default=3)
    parser.add_argument('--output', type=Path, default=ROOT / 'tests/fixtures/interpolation_nul.json')
    args = parser.parse_args()
    matrix = [(*item, args.ruby27) for item in itertools.product([('2.7', 'parser_whitequark'), ('3.3', 'parser_whitequark'), ('3.4', 'parser_prism')], ['single_quotes', 'double_quotes'], ['lf', 'crlf'], ['inspect', '-a', '-A'])]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        records = list(pool.map(record, matrix))
    args.output.write_text('[\n' + ',\n'.join(json.dumps(row, ensure_ascii=False, separators=(',', ':')) for row in records) + '\n]\n', encoding='utf-8', newline='')
    print(f'{len(records)} CLI invocations / {sum(len(row["files"]) for row in records)} file conditions')


if __name__ == '__main__':
    main()
