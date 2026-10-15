"""書き戻し中止の終了2を、既存マニフェストのパスに限って照合する。"""

import json
import pathlib
import re
import sys

import ff_gate


def check(stderr, work, manifest, corpus, mode):
    """終了2に別のエラーが混ざっていたら、記録済みの保護動作へ読み替えない。"""
    known = {
        entry["path"]: entry["id"] for entry in ff_gate.manifest_entries(manifest, corpus, mode)
    }
    messages = stderr.read_text().splitlines()
    if not messages:
        raise ValueError("終了2の理由が出力されていない")
    result = []
    for message in messages:
        found = re.fullmatch(
            r"Autocorrection was not written to (.+) because it introduced a syntax error\.",
            message,
        )
        if not found:
            raise ValueError(f"記録済みの書き戻し中止ではない: {message}")
        relative = str(pathlib.Path(found[1]).resolve().relative_to(work.resolve()))
        if relative not in known:
            raise ValueError(f"未登録の書き戻し中止: {relative}")
        result.append({"path": relative, "divergence_id": known[relative]})
    return result


if __name__ == "__main__":
    try:
        print(
            json.dumps(
                check(*(pathlib.Path(arg) for arg in sys.argv[1:4]), *sys.argv[4:6]),
                ensure_ascii=False,
            )
        )
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(2) from error
