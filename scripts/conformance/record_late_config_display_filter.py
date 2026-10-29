"""表示filterの本家4追補を、主38と分けて固定版CLIから再採取する。"""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER
from record_late_config_reports import _environment, _fresh_paths, _inventory, _runtime, _same

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/late_config_display_filter.json"
IDS = (
    "unfiltered-disk-control",
    "filtered-disk",
    "filtered-stdin-file",
    "filtered-stdin-directory",
)
INPUT_SHA256 = "fcd9390d1ed3083a57db09f1b1e5a5200065e4e86c0962f3b793703c5e78945a"


def _template():
    return json.loads(FIXTURE.read_bytes())


def case_inputs():
    """陽性1とfiltered3の元入力を固定する。

    Returns:
        list: 主38へ加算しない4入力。

    Raises:
        ValueError: 固定入力/ID順が変更された場合。

    """
    value = _template()
    rows = [value["positive_control"], *value["cases"]]
    forms = [{key: row[key] for key in ("id", "files", "args", "env", "stdin_hex")} for row in rows]
    digest = hashlib.sha256(
        json.dumps(forms, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if tuple(row["id"] for row in forms) != IDS or digest != INPUT_SHA256:
        raise ValueError("changed display-filter inputs")
    return forms


def _report(raw, expected):
    value = json.loads(raw)
    metadata = value.get("metadata", {})
    if (
        metadata.get("ruby_engine") != "ruby"
        or metadata.get("rubocop_version") != "1.89.0"
        or metadata.get("ruby_version") != "4.0.7"
    ):
        raise ValueError("wrong reference identity")
    if [row["path"] for row in value["files"]] != expected["expected_target_paths"]:
        raise ValueError("wrong actual target prefix")
    summary = value["summary"]
    count = len(expected["expected_target_paths"])
    offenses = 1 if expected["id"] == IDS[0] else 0
    if any(
        type(summary.get(key)) is not int
        for key in ("target_file_count", "inspected_file_count", "offense_count")
    ):
        raise ValueError("non-integer completion")
    if summary != {
        "target_file_count": count,
        "inspected_file_count": count,
        "offense_count": offenses,
    }:
        raise ValueError("wrong completion counts")
    if sum(len(row["offenses"]) for row in value["files"]) != offenses:
        raise ValueError("incomplete offense")
    if offenses and (
        value["files"][0]["offenses"][0]["correctable"] is not False
        or value["files"][0]["offenses"][0]["cop_name"] != "Naming/PredicatePrefix"
    ):
        raise ValueError("wrong positive control")


def validate_fixture(value):
    """全raw/型と陽性対照を検査し、filteredの0完了と混同しない。

    Raises:
        ValueError: 実参照の入力/出力/完了/coverageが欠落または変更された場合。

    """
    if not _same(value, _template()):
        raise ValueError("changed real reference")
    rows = [value["positive_control"], *value["cases"]]
    if type(value.get("schema")) is not int or value["schema"] != 1 or len(rows) != 4:
        raise ValueError("wrong schema or incomplete controls")
    for row, form in zip(rows, case_inputs(), strict=True):
        if (
            not _same({key: row[key] for key in form}, form)
            or type(row["exit"]) is not int
            or type(row["out_exists"]) is not bool
            or row["out_exists"]
        ):
            raise ValueError("wrong reference types or out")
        _report(bytes.fromhex(row["stdout_hex"]), row)


def record(ruby, form, work):
    """実CLI4条件を採取し、表示済みreportと終了値を全field照合する。

    Returns:
        dict: 実rawと原本保持を持つ1条件。

    Raises:
        ValueError: 固定入力・原本bytes/mtime/mode・全JSON/終了値が異なる場合。

    """
    forms = case_inputs()
    index = next(
        (index for index, expected in enumerate(forms) if expected.get("id") == form.get("id")),
        None,
    )
    if index is None or not _same(form, forms[index]):
        raise ValueError("changed or unknown input")
    expected = [_template()["positive_control"], *_template()["cases"]][index]
    work = Path(work)
    if os.path.lexists(work):
        raise ValueError("occupied private work")
    work.mkdir(parents=True)
    for name, text in form["files"].items():
        (work / name).write_bytes(text.encode())
    before = _inventory(work, form["files"])
    child = subprocess.run(
        [
            ruby,
            "-e",
            LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
            "--",
            *form["args"],
        ],
        cwd=work,
        env=_environment(form["env"]),
        input=bytes.fromhex(form["stdin_hex"]),
        capture_output=True,
        timeout=60,
        check=False,
    )
    if before != _inventory(work, form["files"]) or (work / "report.json").exists():
        raise ValueError("source/out changed")
    row = {
        **expected,
        "exit": child.returncode,
        "stdout_hex": child.stdout.hex(),
        "stderr_hex": child.stderr.hex(),
    }
    _report(child.stdout, expected)
    if not _same(row, expected):
        raise ValueError("actual reference differs")
    with (work / "private-raw.json").open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(row, handle, ensure_ascii=True, indent=2)
        handle.write("\n")
    return row


def main():
    """候補を起動せず、表示filterの実本家4条件だけを排他保存する。

    Raises:
        ValueError: 保存先・版/source・原本/出力の契約が異なる場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    work, output = _fresh_paths(args.work_dir, args.output)
    validate_fixture(_template())
    if os.name != "nt":
        raise ValueError("actual records require fixed Windows Ruby")
    work.mkdir(parents=True)
    _runtime(args.ruby, work, _template()["sources"])
    rows = [record(args.ruby, form, work / form["id"]) for form in case_inputs()]
    result = {**_template(), "positive_control": rows[0], "cases": rows[1:]}
    validate_fixture(result)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(result, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
