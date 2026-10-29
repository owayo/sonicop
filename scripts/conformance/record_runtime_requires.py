"""実RuboCopだけでrequire4CLI/4APIと別scope25境界を再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

ROOT = Path(__file__).resolve().parents[2]
MAIN = ROOT / "tests/fixtures/runtime_requires.json"
BOUNDARIES = ROOT / "tests/fixtures/runtime_requires_boundaries.json"
MAIN_IDS = ("relative-empty", "exact-empty", "relative-registered", "exact-registered")
BOUNDARY_IDS = (
    "once-inherit",
    "order-inherit",
    "cli-before-config",
    "cli-version",
    "config-stdin",
    "cli-stdin",
    "dynamic-bare",
    "late-bare",
    "nested-only",
    "nested-except",
    "root-first-only",
    "exit-seven",
    "abort-once",
    "interrupt",
    "at-exit-seven",
    "at-exit-error",
    "at-exit-after-error",
    "default-getter",
    "default-setter",
    "config-plugin",
    "cli-plugin",
    "cli-stdin-partial",
    "inherited-fatal-before-later-require",
    "inherited-relative-origin",
    "dynamic-ambiguous",
)
FORM_HASHES = (
    "622e663b3d0a90f4b09b6ed668e5554aaaa6a52abc92adb57806e1a40146a1d4",
    "a980d4e73ee8c360f56f3ada4037d9e22773e574ac83cb273f96fc6afef057b3",
)

PRIMARY = {
    "ruby": "4.0.7",
    "versions": {
        "parser": "3.3.12.0",
        "prism": "1.8.1",
        "rubocop-ast": "1.50.0",
        "rubocop": "1.89.0",
        "rainbow": "3.1.1",
    },
    "sources": {
        "lib/rubocop/feature_loader.rb": "a3fb5f43296d24050c70652769e2b4016fd51e09df232c861ccb24e68e07a5c4",
        "lib/rubocop/config_loader.rb": "90dd87be340fd22a81f7205ac0dbc4315f5a9eba08d3f3aef80f0fde3394f38e",
        "lib/rubocop/config_loader_resolver.rb": "f8c40f5e08ad81ba25b76b8296e93ebd766af12730ab4d2bea770710d963c485",
        "lib/rubocop/config_obsoletion/extracted_cop.rb": "630578e5ca56661230b80fc6c237a1ed9799973a41883a173e59a4ac612845ca",
        "lib/rubocop/cop/registry.rb": "41af570b3bbf8f3d28778e18be26d69130f5d23006327783d4b22e57e684fff7",
        "lib/rubocop/options.rb": "adfe551344fc98ea2d13f21852b56ff1cd445a6fdf36cd0bde8cd2a154a4d8c4",
        "lib/rubocop/plugin.rb": "ae78449e009658bc0cde1974ecf2faecc9f59c8819d93043278571f3c368601b",
        "lib/rubocop/plugin/loader.rb": "43140f97e80695d72bb1ed98106e6fda37c267eb38f4fac595cd837ebef24649",
    },
    "reference_sha256": "d58945a26d614706ebed4df7ce7f7b44738e716cc9f9a66ace389e5206f249bb",
}
PINNED_LOADER = LOADER.replace("require 'rubocop'", "gem 'lint_roller', '1.1.0'\nrequire 'rubocop'")
CLI = PINNED_LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"
API = (
    PINNED_LOADER
    + """
require 'json'
RuboCop::ConfigLoader.clear_options
error = nil
begin
  RuboCop::ConfigLoader.load_file('config.yml')
rescue Exception => exception
  error = {class: exception.class.name, message: exception.message}
end
print JSON.generate({loaded_features: RuboCop::ConfigLoader.loaded_features.to_a,
 loaded_plugins: RuboCop::ConfigLoader.loaded_plugins.map { |plugin| plugin.about.name },
 performance_names: RuboCop::Cop::Registry.global.names.grep(/^Performance\\//), error: error})
"""
)


def _same(actual, expected):
    if type(actual) is not type(expected):
        raise ValueError("参照fieldの型が変わりました")
    if isinstance(expected, dict):
        if actual.keys() != expected.keys():
            raise ValueError("参照field集合が変わりました")
        for key, value in expected.items():
            _same(actual[key], value)
    elif isinstance(expected, list):
        if len(actual) != len(expected):
            raise ValueError("参照件数が変わりました")
        for left, right in zip(actual, expected, strict=True):
            _same(left, right)
    elif actual != expected:
        raise ValueError(f"参照fieldが変わりました: {actual!r} != {expected!r}")


def _forms(data):
    return [
        {
            key: row[key]
            for key in ("id", "files", "args", "env", "ruby_args", "stdin")
            if key in row
        }
        for row in data["cases"]
    ]


def validate_fixture(data, *, boundaries=False):
    """固定入力/ID/型/全期待fieldを検査し、空fixtureを成功にしない。

    Args:
        data: 公開参照fixture。
        boundaries: 別scope25条件なら真。

    Raises:
        ValueError: 入力又は参照の契約が変わった場合。

    """
    if type(data.get("schema")) is not int or data["schema"] != 1:
        raise ValueError("unsupported schema")
    if (
        data.get("reference_only") is not True
        or type(data.get("native_invocations")) is not int
        or data["native_invocations"] != 0
    ):
        raise ValueError("候補の出力を採取に混ぜません")
    _same(data.get("runtime" if boundaries else "primary"), PRIMARY)
    if not boundaries and (data.get("ruby") != "4.0.7" or data.get("rubocop") != "1.89.0"):
        raise ValueError("固定Ruby/RuboCop版が必要です")
    expected_ids = BOUNDARY_IDS if boundaries else MAIN_IDS
    if [row.get("id") for row in data.get("cases", [])] != list(expected_ids):
        raise ValueError("固定ID/順序/件数が必要です")
    digest = hashlib.sha256(
        json.dumps(_forms(data), sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != FORM_HASHES[int(boundaries)]:
        raise ValueError("固定入力が変わりました")
    for row in data["cases"]:
        cli = row if boundaries else row["cli"]
        if type(cli.get("exit")) is not int:
            raise ValueError("exitは整数です")
        for key in ("stdout_hex", "stderr_hex"):
            if not isinstance(cli.get(key), str):
                raise ValueError("raw hexが必要です")
            bytes.fromhex(cli[key])
        if boundaries:
            scope = (
                "native_cop_boundary"
                if row["id"] == "nested-only"
                else "version_identity"
                if row["id"] in {"cli-version", "at-exit-seven", "at-exit-error"}
                else "strict"
            )
            if row.get("comparison") != scope:
                raise ValueError("比較範囲を他の行へ移せません")
        else:
            if cli.get("inputs_unchanged") is not True:
                raise ValueError("原本保持の観測が必要です")
            api = row["api"]
            for key in ("loaded_features", "loaded_plugins", "performance_names"):
                if not isinstance(api.get(key), list) or any(
                    type(value) is not str for value in api[key]
                ):
                    raise ValueError("実登録metadataが必要です")


def child_environment(extra):
    """共用ENV4とambient色を子copyから除き、GEM/OS/親を保つ。

    Args:
        extra: 固定caseの環境。

    Returns:
        dict: 実RuboCopの子環境。

    """
    environment = reference_environment()
    for key in list(environment):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            environment.pop(key)
    return {**environment, **extra}


def _inventory(root, files):
    result = {}
    for name in files:
        path = root / name
        info = path.stat()
        result[name] = (path.read_bytes(), info.st_mtime_ns, info.st_mode)
    return result


def _encoded_inventory(inventory):
    return {
        name: {"bytes_hex": value[0].hex(), "mtime_ns": value[1], "mode": value[2]}
        for name, value in inventory.items()
    }


def _private(root, name, output, before, after):
    value = {
        "exit": output.returncode,
        "stdout_hex": output.stdout.hex(),
        "stderr_hex": output.stderr.hex(),
        "inputs_before": _encoded_inventory(before),
        "out_before": None,
        "inputs_after": _encoded_inventory(after),
        "out_after": _encoded_inventory(_inventory(root, ["report.json"]))
        if (root / "report.json").exists()
        else None,
    }
    with (root / name).open("x", encoding="utf8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=True, indent=2) + "\n")


def _write_inputs(root, form):
    root.mkdir()
    for name, value in form["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(value.encode())
    return _inventory(root, form["files"])


def _run(ruby, root, form, source, args):
    output = subprocess.run(
        [str(ruby), *form["ruby_args"], "-e", source, "--", *args],
        cwd=root,
        env=child_environment(form["env"]),
        input=(form.get("stdin") or "").encode(),
        capture_output=True,
        timeout=60,
        check=False,
    )
    return output


def _project(value, root):
    # 実cwdの測定済みprefixだけを投影し、Windows短縮表記をcanonicalへ変えない。
    physical = root.resolve() if sys.platform == "darwin" else root.absolute()
    prefix = re.escape(physical.as_posix().encode())
    pattern = rb"(?<![A-Za-z0-9_./\\:-])" + prefix + rb"(?=/|$|[\s\"':])"
    return re.sub(pattern, b"<root>", value)


def record(ruby, root, expected):
    """元4CLIを全raw/終了値/原本属性で照合する。

    Args:
        ruby: 実Ruby。
        root: fresh case directory。
        expected: 固定参照行。

    Returns:
        dict: 実測した参照行。

    Raises:
        ValueError: 結果又は原本が変わった場合。

    """
    if expected["id"] not in MAIN_IDS:
        raise ValueError("unknown case")
    _same(
        expected,
        next(row for row in json.loads(MAIN.read_bytes())["cases"] if row["id"] == expected["id"]),
    )
    before = _write_inputs(root, expected)
    output = _run(ruby, root, expected, CLI, expected["args"])
    after = _inventory(root, expected["files"])
    _private(root, "private-cli-raw.json", output, before, after)
    if after != before or (root / "report.json").exists():
        raise ValueError("CLIが原本/出力先を変えました")
    result = copy.deepcopy(expected)
    result["cli"] = {
        "exit": output.returncode,
        "stdout_hex": output.stdout.hex(),
        "stderr_hex": output.stderr.hex(),
        "inputs_unchanged": True,
    }
    _same(result["cli"], expected["cli"])
    return result


def record_api(ruby, root, expected):
    """同じ4入力の実ConfigLoader APIを別プロセスで観測する。

    Args:
        ruby: 実Ruby。
        root: CLI入力を保持したcase directory。
        expected: 固定参照行。

    Returns:
        dict: 実登録/feature/例外のmetadata。

    Raises:
        ValueError: 結果又は原本が変わった場合。

    """
    _same(
        expected,
        next(row for row in json.loads(MAIN.read_bytes())["cases"] if row["id"] == expected["id"]),
    )
    before = _inventory(root, expected["files"])
    if os.path.lexists(root / "report.json"):
        raise ValueError("API前に存在しないoutが必要です")
    output = _run(ruby, root, expected, API, [])
    after = _inventory(root, expected["files"])
    _private(root, "private-api-raw.json", output, before, after)
    if (
        output.returncode != 0
        or output.stderr
        or (root / "report.json").exists()
        or after != before
    ):
        raise ValueError("API観測が未完了又は原本/出力先が変わりました")
    result = json.loads(output.stdout)
    _same(result, expected["api"])
    return result


def record_boundaries(ruby, root, expected):
    """元4へ加算せず、測定した25境界を再採取する。

    Args:
        ruby: 実Ruby。
        root: fresh case directory。
        expected: 固定参照行。

    Returns:
        dict: cwd限定投影後の実raw行。

    Raises:
        ValueError: 結果又は原本が変わった場合。

    """
    _same(
        expected,
        next(
            row
            for row in json.loads(BOUNDARIES.read_bytes())["cases"]
            if row["id"] == expected["id"]
        ),
    )
    before = _write_inputs(root, expected)
    output = _run(ruby, root, expected, CLI, expected["args"])
    after = _inventory(root, expected["files"])
    _private(root, "private-cli-raw.json", output, before, after)
    if after != before or (root / "report.json").exists():
        raise ValueError("境界CLIが原本/出力先を変えました")
    result = copy.deepcopy(expected)
    result.update(
        exit=output.returncode,
        stdout_hex=_project(output.stdout, root).hex(),
        stderr_hex=_project(output.stderr, root).hex(),
    )
    _same(result, expected)
    return result


def _fresh(paths):
    identities = []
    for path in paths:
        if os.path.lexists(path):
            raise ValueError("保存先entryは既に存在します")
        identities.append(os.path.normcase(os.path.abspath(path.resolve(strict=False))))
    if len(set(identities)) != len(paths):
        raise ValueError("保存先が別表記で重複しています")


def _primary(ruby, work, expected):
    source = (
        PINNED_LOADER
        + "\nrequire 'json'; require 'digest'\n"
        + """
files = JSON.parse(ARGV.fetch(0))
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
print JSON.generate({ruby: RUBY_VERSION, versions: Gem.loaded_specs.slice(*JSON.parse(ARGV.fetch(1))).transform_values { |spec| spec.version.to_s }, sources: files.to_h { |name| [name, Digest::SHA256.file(File.join(root,name)).hexdigest] }, reference_sha256: ARGV.fetch(2)})
"""
    )
    output = subprocess.run(
        [
            str(ruby),
            "-e",
            source,
            json.dumps(list(expected["sources"])),
            json.dumps(list(expected["versions"])),
            expected["reference_sha256"],
        ],
        cwd=work,
        env=child_environment({}),
        capture_output=True,
        timeout=60,
        check=False,
    )
    if output.returncode != 0 or output.stderr:
        raise ValueError("固定runtime/source照会に失敗しました")
    result = json.loads(output.stdout)
    _same(result, expected)
    _same({key: result["versions"][key] for key in VERSIONS}, VERSIONS)
    return result


def collect(ruby, work, output, boundaries_output):
    """全件完了後だけ、二つの公開保存先をexclusive作成する。

    Args:
        ruby: 固定gemが導入された実Ruby。
        work: 未作成のprivate work。
        output: 元4CLI/4APIの保存先。
        boundaries_output: 別scope25条件の保存先。

    Raises:
        ValueError: fresh/参照/runtime/保持契約を満たさない場合。

    """
    _fresh([work, output, boundaries_output])
    if os.name != "nt":
        raise ValueError("公開rawはWindows実RuboCopから再採取します")
    main = json.loads(MAIN.read_bytes())
    boundaries = json.loads(BOUNDARIES.read_bytes())
    validate_fixture(main)
    validate_fixture(boundaries, boundaries=True)
    work.mkdir(parents=True)
    main["primary"] = _primary(ruby, work, main["primary"])
    _same(main["primary"], boundaries["runtime"])
    for index, expected in enumerate(main["cases"]):
        root = work / expected["id"]
        row = record(ruby, root, expected)
        row["api"] = record_api(ruby, root, expected)
        main["cases"][index] = row
    boundaries["cases"] = [
        record_boundaries(ruby, work / row["id"], row) for row in boundaries["cases"]
    ]
    for path, data in ((output, main), (boundaries_output, boundaries)):
        with path.open("x", encoding="utf8", newline="\n") as stream:
            stream.write(json.dumps(data, ensure_ascii=True, indent=2) + "\n")


def main():
    """候補への引数を受け取らず、固定版の実RuboCopだけを再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--boundaries-output", type=Path, required=True)
    arguments = parser.parse_args()
    collect(arguments.ruby, arguments.work, arguments.output, arguments.boundaries_output)


if __name__ == "__main__":
    main()
