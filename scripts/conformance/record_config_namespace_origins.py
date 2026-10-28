"""Windowsの設定originを本家24条件だけから再採取する。"""

import argparse
import ctypes
import hashlib
import json
import ntpath
import os
import subprocess
from pathlib import Path, PureWindowsPath

import record_config_cop_namespaces as common

TEMPLATE = Path(__file__).parents[2] / "tests/fixtures/config_cop_namespace_origins.json"
CASE_IDS = {
    "explicit": [
        "long_relative",
        "short_relative",
        "long_absolute_long",
        "short_absolute_short",
        "short_absolute_long",
        "long_absolute_short",
        "short_relative_inherit",
        "short_absolute_long_inherit",
        "short_relative_ambiguous",
        "short_absolute_long_ambiguous",
        "aliases_short_long",
        "aliases_long_short",
        "uppercase_filename",
        "short_filename",
    ],
    "discovery": [
        "discovery_long_relative",
        "discovery_short_relative",
        "discovery_short_target_long",
        "discovery_long_target_short",
        "explicit_short_long",
        "explicit_long_short",
        "root_long_relative",
        "root_short_relative",
        "root_short_target_long",
        "root_long_target_short",
    ],
}


def _validate_template(value):
    common._require(value["schema"] == 1, "未知のschema")
    common._require([group["id"] for group in value["groups"]] == list(CASE_IDS), "groupが不完全")
    for group in value["groups"]:
        common._require(
            [case["id"] for case in group["cases"]] == CASE_IDS[group["id"]],
            "条件が空・未知・重複・欠落・順序違い",
        )


def _transform(value, replacements):
    if isinstance(value, dict):
        return {key: _transform(item, replacements) for key, item in value.items()}
    if isinstance(value, list):
        return [_transform(item, replacements) for item in value]
    if isinstance(value, str):
        for original, replacement in replacements.items():
            value = value.replace(original, replacement)
    return value


def _windows_spelling(path, procedure):
    function = getattr(ctypes.WinDLL("kernel32", use_last_error=True), procedure)
    function.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint]
    function.restype = ctypes.c_uint
    output = ctypes.create_unicode_buffer(32_768)
    length = function(str(path), output, len(output))
    if not 0 < length < len(output):
        raise OSError(ctypes.get_last_error(), procedure)
    return output.value.replace("\\", "/")


def _roots(directory):
    long = _windows_spelling(directory, "GetLongPathNameW")
    short = _windows_spelling(directory, "GetShortPathNameW")
    hybrid = (PureWindowsPath(short).parent / PureWindowsPath(long).name).as_posix()
    common._require(len({long, short, hybrid}) == 3, "親と末尾の両方に8.3 aliasが必要")
    return {"<long>": long, "<short>": short, "<hybrid>": hybrid}


def _target(cwd, argument):
    target = PureWindowsPath(ntpath.normpath(str(PureWindowsPath(cwd) / argument))).as_posix()
    prefix = PureWindowsPath(cwd).as_posix() + "/"
    return target.removeprefix(prefix)


def _collect(ruby, work, template):
    api = common._registry(ruby, work)
    launcher = work / "reference.rb"
    launcher.write_text(
        common.LOADER + "\nload Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    runtime = {
        "ruby": api["runtime"]["ruby"],
        "rubocop": api["runtime"]["gems"]["rubocop"],
        "platform": api["runtime"]["platform"],
    }
    common._require(runtime == template["runtime"], "実runtimeが不一致")
    result = {"schema": 1, "runtime": runtime, "long_filename": template["long_filename"]}
    result["groups"] = []
    for group in template["groups"]:
        directory = work / (group["id"] + "-namespace-origin-directory")
        directory.mkdir()
        roots = _roots(directory)
        for name, source in group["files"].items():
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(_transform(source, roots).encode())
        substitutions = dict(roots)
        if group["id"] == "explicit":
            substitutions["<short_file>"] = PureWindowsPath(
                _windows_spelling(directory / template["long_filename"], "GetShortPathNameW")
            ).name
        project = {path: token for token, path in roots.items()}
        rows = []
        for case in group["cases"]:
            args = _transform(case["args"], substitutions)
            cwd = _transform(case["cwd"], substitutions)
            environment = common.reference_environment()
            environment["CLICOLOR_FORCE"] = "0"
            done = subprocess.run(
                [ruby, str(launcher), *args],
                cwd=cwd,
                env=environment,
                capture_output=True,
                check=False,
                timeout=60,
            )
            raw = {
                "exit": done.returncode,
                "stdout_hex": done.stdout.hex(),
                "stderr_hex": done.stderr.hex(),
            }
            with (directory / (case["id"] + ".raw.json")).open(
                "x", encoding="utf-8", newline="\n"
            ) as stream:
                stream.write(json.dumps(raw, indent=2) + "\n")
            if "ambiguous" in case["id"]:
                common._require(
                    done.returncode == 2 and not done.stdout and done.stderr, "曖昧名拒否が不完全"
                )
                report = None
            else:
                common._require(done.returncode in {0, 1}, "通常検査が失敗")
                report = common._report(done.stdout, [_target(cwd, args[-1])])
            for name, source in group["files"].items():
                common._require(
                    (directory / name).read_bytes() == _transform(source, roots).encode(),
                    "原本が変化した",
                )
            rows.append(
                {
                    "id": case["id"],
                    "cwd": case["cwd"],
                    "args": case["args"],
                    "exit": done.returncode,
                    "report": _transform(report, project),
                    "stderr_lf": _transform(done.stderr.decode().replace("\r\n", "\n"), project),
                    "raw_stdout_sha256": hashlib.sha256(done.stdout).hexdigest(),
                    "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
                }
            )
        result["groups"].append({"id": group["id"], "files": group["files"], "cases": rows})
    _validate_template(result)
    return result


def main():
    """既存entryを拒否し、候補を起動せず全24件が完了した本家記録だけを新規保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    work, output = (Path(value).absolute() for value in (args.work, args.out))
    common._require(
        work != output and not any(os.path.lexists(path) for path in (work, output)),
        "既存work/outを上書きしない",
    )
    template = json.loads(TEMPLATE.read_text(encoding="utf-8"))
    _validate_template(template)
    common._require(os.name == "nt", "実Windows環境が必要")
    work.mkdir(parents=True, exist_ok=False)
    result = _collect(args.ruby, work, template)
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print("recorded 24 Windows origin cases; 22 JSON / 2 ambiguity bodies")


if __name__ == "__main__":
    main()
