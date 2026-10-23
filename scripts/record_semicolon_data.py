#!/usr/bin/env python3
"""RuboCop 1.89.0 の DATA 除外を CLI と MRI の DATA.read で記録する。"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CASES = {
    "original": ("a=1;\n__END__\nkept;\n", False),
    "data_without_code_semicolon": ("STDOUT.write(DATA.read)\n__END__\n;;;\n", True),
    "code_terminator": ("STDOUT.write(DATA.read);\n__END__\nkept;\n", True),
    "data_multiple_lines": ("STDOUT.write(DATA.read);\n__END__\n; first;\n# ;\n};\n", True),
    "empty_data": ("STDOUT.write(DATA.read);\n__END__\n", True),
    "data_without_final_newline": ("STDOUT.write(DATA.read);\n__END__\nkept;", True),
    "unicode_data": ("STDOUT.write(DATA.read);\n__END__\n日本語; é;\n", True),
    "data_second_marker": ("STDOUT.write(DATA.read);\n__END__\n__END__;\nkept;;\n", True),
    "data_soh": ("STDOUT.write(DATA.read);\n__END__\n;\x01;\n", True),
    "code_separator": ("x=1; STDOUT.write(DATA.read)\n__END__\nkept;\n", True),
    "code_trailing_comment": ("STDOUT.write(DATA.read); # note\n__END__\nkept;\n", True),
    "quoted_marker": ('s="__END__;";\n', False),
    "comment_marker": ("x=1; # __END__ ;\n", False),
    "heredoc_marker": ("s = <<~TEXT\n__END__\nkept;\nTEXT\ns;\n", False),
    "similar_identifier": ("__END__x;\n", False),
    "ordinary_code": ("a=1; b=2;\n", False),
    "percent_literal": ("s=%q(__END__; kept;);\ns;\n", False),
    "comment_before_data": ("STDOUT.write(DATA.read)\n# __END__ ;\n__END__\nkept;\n", True),
}


def data_read(ruby, folder, path):
    result = subprocess.run([str(ruby), '-W0', path], cwd=folder, capture_output=True, timeout=30)
    if result.returncode or result.stderr:
        raise RuntimeError(result.stderr.decode(errors='replace'))
    return result.stdout.hex()


def preserve_crlf(raw):
    return raw.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')


def record(ruby, version, engine, ending, mode):
    config = f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {version}\n  ParserEngine: {engine}\nStyle/Semicolon:\n  Enabled: true\n'
    with tempfile.TemporaryDirectory(prefix='semicolon-data-reference-') as temporary:
        folder = Path(temporary)
        (folder / '.rubocop.yml').write_text(config, encoding='utf-8', newline='')
        files = []
        for index, (name, (text, runtime)) in enumerate(CASES.items()):
            raw = text.encode()
            if ending == 'crlf':
                raw = preserve_crlf(raw)
            path = f'case{index:03}.rb'
            (folder / path).write_bytes(raw)
            before = data_read(ruby, folder, path) if runtime else None
            files.append({'id': name, 'path': path, 'source_hex': raw.hex(), 'data_before_hex': before})
        command = ['rubocop', '_1.89.0_', '--raise-cop-error', '--cache', 'false', '--format', 'json', '--only', 'Style/Semicolon']
        if mode != 'inspect':
            command.append(mode)
        command += [case['path'] for case in files]
        outputs = []
        for run in range(2):
            result = subprocess.run(command, cwd=folder, capture_output=True, timeout=90)
            if result.returncode not in (0, 1) or result.stderr:
                raise RuntimeError(result.stderr.decode(errors='replace'))
            report = json.loads(result.stdout)
            assert report['metadata']['rubocop_version'] == '1.89.0'
            assert report['metadata']['ruby_version'] == '4.0.8'
            assert report['summary']['target_file_count'] == report['summary']['inspected_file_count'] == len(files)
            assert {file['path'] for file in report['files']} == {file['path'] for file in files}
            outputs.append({'exit': result.returncode, 'report': report})
            for case in files:
                raw = (folder / case['path']).read_bytes()
                expected = preserve_crlf(raw) if ending == 'crlf' else raw
                suffix = 'first' if run == 0 else 'rerun'
                case[suffix + '_raw_reference_after_hex'] = raw.hex()
                case[suffix + '_after_hex'] = expected.hex()
                if case['data_before_hex'] is not None:
                    case[suffix + '_raw_reference_data_hex'] = data_read(ruby, folder, case['path'])
                    # 本家の改行正規化と既存の CRLF 保持方針を区別し、期待する disk 自体も実行する。
                    (folder / case['path']).write_bytes(expected)
                    case[suffix + '_expected_data_hex'] = data_read(ruby, folder, case['path'])
                    assert case[suffix + '_expected_data_hex'] == case['data_before_hex']
                    (folder / case['path']).write_bytes(raw)
    return {'target': version + '/' + engine, 'ending': ending, 'mode': mode, 'config': config, 'files': files, 'first': outputs[0], 'rerun': outputs[1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ruby27', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'tests/fixtures/semicolon_data.json')
    args = parser.parse_args()
    runtime = subprocess.run([str(args.ruby27), '-v'], capture_output=True, text=True, check=True)
    assert 'ruby 2.7.8' in runtime.stdout
    groups = [record(args.ruby27, version, engine, ending, mode)
              for version, engine in [('2.7', 'parser_whitequark'), ('3.4', 'parser_prism')]
              for ending in ['lf', 'crlf'] for mode in ['inspect', '-a', '-A']]
    args.output.write_text('[\n' + ',\n'.join(json.dumps(group, ensure_ascii=False, separators=(',', ':')) for group in groups) + '\n]\n', encoding='utf-8', newline='')
    print(f'{len(groups)} CLI groups, {sum(len(group["files"]) for group in groups)} file conditions plus reruns; DATA.read verified with MRI 2.7.8')


if __name__ == '__main__':
    main()
