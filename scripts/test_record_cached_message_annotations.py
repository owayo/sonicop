#!/usr/bin/env python3
"""参照 collector の子環境隔離と親環境保持の契約。"""

import hashlib
import json
import os
import unittest
from pathlib import Path
from unittest.mock import patch

import record_cached_message_annotations as recorder
from record_cached_message_annotations import _printable_table, _ruby_environment


class ReferenceEnvironmentTest(unittest.TestCase):
    """Ruby 起動へ渡す copy と親の環境を分けて検査する。"""

    def test_removes_only_four_child_overrides_and_keeps_parent(self):
        """4キーだけを除去し、Gem/PATH/OS と親辞書の値を保つ。"""
        # GEM/PATH/OS の探索契約まで消すと、別の参照 Ruby を起動してしまう。
        parent = {
            "RUBYOPT": "-rpoison.rb",
            "RUBYLIB": "poison-load-path",
            "RUBOCOP_OPTS": "--unknown-poison-option",
            "RUBOCOP_TARGET_RUBY_VERSION": "0.1",
            "GEM_HOME": "expected-gems",
            "GEM_PATH": "expected-gem-search",
            "PATH": "expected-tool-path",
            "SystemRoot": "expected-os-root",
            "TMP": "expected-temp-root",
            "KEEP_SENTINEL": "expected-user-value",
        }
        with patch.dict(os.environ, parent, clear=True):
            before = dict(os.environ)
            child = _ruby_environment()
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(
                child,
                {
                    key: value
                    for key, value in before.items()
                    if key
                    not in {
                        "RUBYOPT",
                        "RUBYLIB",
                        "RUBOCOP_OPTS",
                        "RUBOCOP_TARGET_RUBY_VERSION",
                    }
                },
            )
            child["KEEP_SENTINEL"] = "child-only-change"
            self.assertEqual(dict(os.environ), before)
        with patch.dict(os.environ, {"PATH": "unpoisoned"}, clear=True):
            self.assertEqual(_ruby_environment(), {"PATH": "unpoisoned"})

    def test_generated_table_matches_recorded_origin_and_hash(self):
        """本家で記録した区間から、LF固定の生成物を byte 単位で再現する。"""
        root = Path(__file__).resolve().parent.parent
        fixture = json.loads(
            (
                root / "tests/fixtures/cached_message_annotations_windows.json"
            ).read_bytes()
        )
        generated = _printable_table(fixture["inspect_keys"])
        self.assertEqual(
            (root / "src/cache_string_printable_table.rs").read_bytes(), generated
        )
        self.assertEqual(
            hashlib.sha256(generated).hexdigest(), fixture["inspect_table_sha256"]
        )


class PublicationTest(unittest.TestCase):
    """二出力の所有と予約を、実Rubyを起動せずに検査する。"""

    def original_bytes(self):
        """期待値を作らず、正式fixtureと生成表のbytesを保存入力に使う。"""
        root = Path(__file__).resolve().parent.parent
        return (
            (
                root / "tests/fixtures/cached_message_annotations_windows.json"
            ).read_bytes(),
            (root / "src/cache_string_printable_table.rs").read_bytes(),
        )

    def invoke(self, root, slots, extra=(), child=None, collection=None):
        """mainの境界を通し、予約前の採取やscratch作成を検出する。"""
        import contextlib
        import io
        import tempfile

        temporary = tempfile.TemporaryDirectory

        def scratch(*args, **kwargs):
            """観測用scratchもtestの新namespaceへ閉じる。"""
            return temporary(*args, dir=root, **kwargs)

        args = ["recorder", "--ruby", "unused-ruby"]
        for flag, path in zip(("--output", "--table-output"), slots, strict=True):
            if path is not None:
                args += [flag, str(path)]
        stdout = io.StringIO()
        stdout.buffer = io.BytesIO()
        failure = None
        with contextlib.ExitStack() as stack:
            for manager in extra:
                stack.enter_context(manager)
            stack.enter_context(patch.object(recorder.sys, "argv", args))
            stack.enter_context(
                patch.object(recorder.shutil, "which", return_value="unused-ruby")
            )
            stack.enter_context(patch.object(recorder.sys, "stdout", stdout))
            work = stack.enter_context(
                patch.object(
                    recorder.tempfile, "TemporaryDirectory", side_effect=scratch
                )
            )
            children = stack.enter_context(
                patch.object(
                    recorder.subprocess,
                    "run",
                    side_effect=child or RuntimeError("unexpected reference child"),
                )
            )
            collected = None
            if collection is not None:
                collected = stack.enter_context(
                    patch.object(
                        recorder, "_collect", side_effect=collection, create=True
                    )
                )
            try:
                recorder.main()
            except Exception as error:
                failure = error
        return {
            "failure": failure,
            "work": work.call_count,
            "children": children.call_count,
            "collect": None if collected is None else collected.call_count,
            "stdout": stdout.getvalue(),
            "stdout_bytes": stdout.buffer.getvalue(),
        }

    def assert_preflight(self, result, error):
        """占有や競合を子・観測用workより前で拒否したことを要求する。"""
        self.assertIsInstance(result["failure"], error)
        self.assertEqual(result["children"], 0)
        self.assertEqual(result["work"], 0)
        self.assertEqual(result["stdout"], "")
        self.assertEqual(result["stdout_bytes"], b"")

    def test_either_occupied_slot_prevents_both_reservations_and_children(self):
        """後ろのslotが占有でも、先の新slotを作らない。"""
        import tempfile

        for index in (0, 1):
            for kind in ("file", "directory"):
                with (
                    self.subTest(index=index, kind=kind),
                    tempfile.TemporaryDirectory() as folder,
                ):
                    root = Path(folder)
                    slots = (root / "fixture.json", root / "table.rs")
                    occupied = slots[index]
                    if kind == "directory":
                        occupied.mkdir()
                        sentinel = occupied / "sentinel"
                    else:
                        sentinel = occupied
                    sentinel.write_bytes(b"foreign\x00\r\n")
                    before = (sentinel.read_bytes(), sentinel.stat().st_mtime_ns)
                    result = self.invoke(root, slots)
                    self.assert_preflight(result, ValueError)
                    self.assertFalse(slots[1 - index].exists())
                    self.assertEqual(
                        (sentinel.read_bytes(), sentinel.stat().st_mtime_ns), before
                    )

    def test_dangling_leaf_is_checked_before_resolution_or_other_reservation(self):
        """実OS symlinkを仮定せず、元leafのAPI占有を両slotで検査する。"""
        import tempfile

        for index in (0, 1):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                slots = (root / "fixture.json", root / "table.rs")
                occupied = slots[index]
                target = root / "missing-target"
                exists, symlink, resolve = Path.exists, Path.is_symlink, Path.resolve
                resolved = []

                def virtual_exists(path, occupied=occupied, exists=exists):
                    return False if path == occupied else exists(path)

                def virtual_symlink(path, occupied=occupied, symlink=symlink):
                    return True if path == occupied else symlink(path)

                def virtual_resolve(
                    path,
                    *args,
                    occupied=occupied,
                    target=target,
                    resolved=resolved,
                    resolve=resolve,
                    **kwargs,
                ):
                    if path == occupied:
                        resolved.append(path)
                        return target
                    return resolve(path, *args, **kwargs)

                result = self.invoke(
                    root,
                    slots,
                    (
                        patch.object(Path, "exists", virtual_exists),
                        patch.object(Path, "is_symlink", virtual_symlink),
                        patch.object(Path, "resolve", virtual_resolve),
                    ),
                )
                self.assert_preflight(result, ValueError)
                self.assertEqual(resolved, [])
                self.assertEqual(list(root.iterdir()), [])

    def test_duplicate_slots_are_rejected_before_any_file_is_created(self):
        """同名・字句上のdot別名を、排他open後の失敗まで持ち越さない。"""
        import tempfile

        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            output = root / "fixture.json"
            aliases = [output, root / "missing" / ".." / output.name]
            if os.name == "nt":
                aliases.append(root / output.name.upper())
            for alias in aliases:
                with self.subTest(alias=alias):
                    result = self.invoke(root, (output, alias))
                    self.assert_preflight(result, ValueError)
                    self.assertEqual(list(root.iterdir()), [])

    def test_reservation_races_keep_peer_and_prior_owned_reservation(self):
        """2slot目の競合でも先の0B予約を消さず、相手bytesを保つ。"""
        import tempfile

        for index in (0, 1):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                slots = (root / "fixture.json", root / "table.rs")
                raced = slots[index]
                original = Path.open
                peer = []

                def racing_open(
                    path,
                    mode="r",
                    *args,
                    raced=raced,
                    original=original,
                    peer=peer,
                    **kwargs,
                ):
                    if path == raced and mode == "xb":
                        with original(path, "wb") as stream:
                            stream.write(b"peer\x00\r\n")
                        peer.append((path.read_bytes(), path.stat().st_mtime_ns))
                    return original(path, mode, *args, **kwargs)

                result = self.invoke(
                    root, slots, (patch.object(Path, "open", racing_open),)
                )
                self.assert_preflight(result, FileExistsError)
                self.assertEqual(len(peer), 1)
                self.assertEqual(
                    (raced.read_bytes(), raced.stat().st_mtime_ns), peer[0]
                )
                if index == 1:
                    self.assertEqual(slots[0].read_bytes(), b"")
                else:
                    self.assertFalse(slots[1].exists())

    def test_child_or_parse_failure_retains_both_empty_reservations(self):
        """実観測本文の最初のchild/parse失敗でも、新規予約を証拠として残す。"""
        import subprocess
        import tempfile

        def invalid_json(command, **kwargs):
            (kwargs["cwd"] / "versions.json").write_bytes(b"{broken")
            return subprocess.CompletedProcess(command, 0, b"{}", b"")

        for child, error in (
            (RuntimeError("child failure"), RuntimeError),
            (invalid_json, json.JSONDecodeError),
        ):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                slots = (root / "fixture.json", root / "table.rs")
                result = self.invoke(root, slots, child=child)
                self.assertIsInstance(result["failure"], error)
                self.assertEqual(result["children"], 1)
                self.assertEqual(result["work"], 1)
                self.assertTrue(all(path.is_file() for path in slots))
                self.assertEqual([path.read_bytes() for path in slots], [b"", b""])
                self.assertEqual(result["stdout_bytes"], b"")

    def test_partial_write_retains_partial_and_other_reserved_or_complete_slot(self):
        """table先行の既存保存順を守り、どちらの途中書込みも成功扱いしない。"""
        import tempfile

        encoded, table = self.original_bytes()
        for index in (0, 1):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                slots = (root / "fixture.json", root / "table.rs")
                failed = slots[index]
                original = Path.open

                class Partial:
                    def __init__(self, stream):
                        self.stream = stream

                    def __enter__(self):
                        return self

                    def __exit__(self, *_):
                        self.stream.close()

                    def write(self, data):
                        self.stream.write(data[:7])
                        self.stream.flush()
                        raise OSError("partial publication")

                def partial_open(
                    path,
                    mode="r",
                    *args,
                    original=original,
                    failed=failed,
                    wrapper=Partial,
                    **kwargs,
                ):
                    stream = original(path, mode, *args, **kwargs)
                    return (
                        wrapper(stream) if path == failed and mode == "xb" else stream
                    )

                result = self.invoke(
                    root,
                    slots,
                    (patch.object(Path, "open", partial_open),),
                    collection=lambda *_: (encoded, table),
                )
                self.assertIsInstance(result["failure"], OSError)
                self.assertEqual(result["children"], 0)
                self.assertEqual(slots[index].read_bytes(), (encoded, table)[index][:7])
                self.assertEqual(
                    slots[1 - index].read_bytes(), table if index == 0 else b""
                )
                self.assertEqual(result["stdout_bytes"], b"")

    def test_all_optional_slot_modes_reserve_first_and_preserve_reference_bytes(self):
        """stdout経路も保ち、生成表/fixture値へ候補出力を流用しない。"""
        import tempfile

        encoded, table = self.original_bytes()
        for fixture_on, table_on in (
            (True, True),
            (True, False),
            (False, True),
            (False, False),
        ):
            with self.subTest(fixture=fixture_on, table=table_on):
                with tempfile.TemporaryDirectory() as folder:
                    root = Path(folder)
                    slots = (
                        root / "new-fixture" / "fixture.json" if fixture_on else None,
                        root / "new-table" / "table.rs" if table_on else None,
                    )
                    reservations = []

                    def collected(
                        options, ruby, slots=slots, reservations=reservations
                    ):
                        reservations.append(
                            all(
                                p.is_file() and p.read_bytes() == b""
                                for p in slots
                                if p
                            )
                        )
                        self.assertEqual(ruby, "unused-ruby")
                        self.assertIsNone(options.sequence)
                        return encoded, table if options.table_output else None

                    result = self.invoke(root, slots, collection=collected)
                    self.assertIsNone(result["failure"])
                    self.assertEqual(result["collect"], 1)
                    self.assertEqual(result["children"], 0)
                    self.assertEqual(result["work"], 0)
                    self.assertEqual(reservations, [True])
                    if fixture_on:
                        self.assertEqual(slots[0].read_bytes(), encoded)
                    if table_on:
                        self.assertEqual(slots[1].read_bytes(), table)
                    self.assertEqual(
                        result["stdout_bytes"], b"" if fixture_on else encoded
                    )


if __name__ == "__main__":
    unittest.main()
