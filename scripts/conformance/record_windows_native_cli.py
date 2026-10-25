"""Windows の native 補正期待を、固定した本家と元 fixture だけから再生成する。"""

import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import subprocess

from record_windows_corrections import execute, oracle_child_env

SPEC_GEMS = {"parser": "3.3.12.0", "prism": "1.8.1", "rubocop-ast": "1.50.0", "rubocop": "1.89.0"}
SET_FIXTURES = {
    "mixed": "mixed_reinspection.json",
    "nul_eol": "nul_line_endings.json",
    "nul_writeback": "nul_writeback_cli.json",
    "nul_semicolon": "nul_semicolon_cli.json",
    "leading": "leading_empty_lines.json",
    "mixed_bare": "mixed_bare_cr.json",
    "pure_preservation": "pure_cr_preservation.json",
}


def pinned_oracle(ruby, out):
    """実 CLI と版照会が同じ固定 gem を読む launcher を生成する。"""
    launcher = out / "reference.rb"
    pins = "\n".join(f"gem '{name}', '{version}'" for name, version in SPEC_GEMS.items())
    launcher.write_text(
        pins
        + "\n"
        + """require 'json'
require 'rubocop'
abort 'Ruby 4.0.7 is required' unless RUBY_VERSION == '4.0.7'
ENV.delete('RUBOCOP_TARGET_RUBY_VERSION')
if ARGV == ['--record-versions']
  names = %w[parser prism rubocop-ast rubocop]
  print JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
    gems: names.to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] }})
else
  load Gem.bin_path('rubocop', 'rubocop', '1.89.0')
end
""",
        encoding="utf-8",
        newline="\n",
    )
    process = subprocess.run(
        [ruby, str(launcher), "--record-versions"],
        capture_output=True,
        check=True,
        timeout=60,
        env=oracle_child_env(),
    )
    if process.stderr:
        raise ValueError(f"unexpected version stderr: {process.stderr!r}")
    versions = json.loads(process.stdout)
    if (
        not isinstance(versions, dict)
        or versions.get("ruby") != "4.0.7"
        or versions.get("gems") != SPEC_GEMS
    ):
        raise ValueError(f"unexpected specification versions: {versions!r}")
    platform = versions.get("platform")
    if not isinstance(platform, str) or not any(name in platform for name in ("mingw", "mswin")):
        raise ValueError("the reference Ruby must run on Windows")
    (out / "loaded-versions.json").write_text(
        json.dumps(versions, indent=2), encoding="utf-8", newline="\n"
    )
    return launcher, versions


def assemble(out, jobs, fixtures, versions):
    """要求した全 job の完了とケース集合を検証し、実測値から期待を組み立てる。"""
    records = {}
    for requested in jobs:
        job = json.loads((out / (requested["name"] + ".json")).read_text(encoding="utf-8"))
        if len(job["records"]) != len(requested["cases"]):
            raise ValueError(f"incomplete job: {requested['name']}")
        for key in ("name", "fixture", "group"):
            if job[key] != requested[key]:
                raise ValueError(f"reference job identity changed: {requested['name']}")
        requested_cases = {case["index"]: case for case in requested["cases"]}
        actual_indices = [case["case_index"] for case in job["records"]]
        if (
            len(requested_cases) != len(requested["cases"])
            or any(type(index) is not int for index in actual_indices)
            or len(set(actual_indices)) != len(actual_indices)
            or set(actual_indices) != set(requested_cases)
        ):
            raise ValueError(f"reference case set is incomplete: {requested['name']}")
        mapping = records.setdefault(job["fixture"], {})
        for case in job["records"]:
            original = requested_cases[case["case_index"]]
            if (
                case["name"] != original["name"]
                or case["source_sha256"] != hashlib.sha256(original["source"]).hexdigest()
            ):
                raise ValueError(
                    f"reference source changed: {requested['name']}/{case['case_index']}"
                )
            if (
                hashlib.sha256(bytes.fromhex(case["safe_after_hex"])).hexdigest()
                != case["safe_after_sha256"]
            ):
                raise ValueError(
                    f"reference output digest disagrees: {requested['name']}/{case['case_index']}"
                )
            key = (
                f"record:{case['case_index']}"
                if job["group"] is None
                else f"group:{job['group']}:case:{case['case_index']}"
            )
            if key in mapping:
                raise ValueError(f"duplicate reference case: {job['fixture']}/{key}")
            expected = {
                name: case[name]
                for name in [
                    "name",
                    "source_sha256",
                    "safe_after_hex",
                    "safe_after_sha256",
                    "expected_first_offenses",
                    "expected_repeated_offenses",
                ]
            }
            expected["expected_first_exit"] = int(
                any(not offense["corrected"] for offense in expected["expected_first_offenses"])
            )
            expected["expected_repeated_exit"] = int(bool(expected["expected_repeated_offenses"]))
            mapping[key] = expected
    if not records or any(not values for values in records.values()):
        raise ValueError("no complete reference cases were measured")
    metadata = dict(versions)
    metadata["fixture_sha256"] = {
        name: hashlib.sha256((fixtures / name).read_bytes()).hexdigest() for name in records
    }
    return {"reference": metadata, "cases": records}


def main():
    """出力先へ Oracle 生記録と期待を保存し、候補の出力は一切読まない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument(
        "--fixtures",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures",
    )
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--sets", default=",".join(SET_FIXTURES))
    parser.add_argument("--check-against", type=pathlib.Path)
    args = parser.parse_args()
    selected = set(args.sets.split(","))
    if not selected or not selected <= SET_FIXTURES.keys():
        parser.error("--sets must name one or more of " + ",".join(SET_FIXTURES))
    fixtures = args.fixtures.resolve()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    launcher, versions = pinned_oracle(args.ruby, out)

    lex_script = out / "tail_boundary.rb"
    lex_script.write_text(
        """require 'json'
    gem 'prism', '1.8.1'
    require 'prism'
    puts JSON.generate(JSON.parse(File.read(ARGV[0])).map do |hex|
      s = [hex].pack('H*').force_encoding('UTF-8')
      tokens = Prism.lex(s).value.map(&:first)
      data = tokens.find { |token| token.type == :__END__ }
      eof = tokens.reverse.find { |token| token.type == :EOF }
      if data
        start = data.location.start_offset
        newline = s.b.index("\\n", start)
        {kind:'DATA',offset:newline ? newline + 1 : s.bytesize}
      elsif eof && eof.location.length.zero? && s.getbyte(eof.location.start_offset) == 0
        {kind:'code-NUL',offset:eof.location.start_offset}
      else
        {kind:nil,offset:s.bytesize}
      end
    end)
    """,
        encoding="utf-8",
        newline="\n",
    )

    def decode(hex_string):
        return bytes.fromhex(hex_string)

    def native(source):
        return source.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")

    def config(ruby, engine, eol=None):
        result = (
            f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {engine}\n"
        )
        if eol:
            result += f"Layout/EndOfLine:\n  EnforcedStyle: {eol}\n"
        return result

    def load(name):
        return json.loads((fixtures / name).read_text(encoding="utf-8"))

    def old_mixed_nul_expected(case):
        source = decode(case["source_hex"])
        reference = decode(case["after_hex"])
        crlf_contents = {
            line[:-2] for line in source.splitlines(keepends=True) if line.endswith(b"\r\n")
        }
        result = b"".join(
            line.rstrip(b"\r\n") + b"\r\n"
            if line.endswith(b"\n") and line.rstrip(b"\r\n") in crlf_contents
            else line
            for line in reference.splitlines(keepends=True)
        )
        if b"\r\n" in source and b"\n" not in source.replace(b"\r\n", b""):
            result = native(result)
        return result

    jobs = []
    if "mixed" in selected:
        rows = load("mixed_reinspection.json")["records"]
        grouped = {}
        for index, row in enumerate(rows):
            grouped.setdefault((row["config"], row["mode"]), []).append((index, row))
        for number, ((yaml, mode), cases) in enumerate(grouped.items()):
            jobs.append(
                {
                    "name": f"mixed-{number:02}",
                    "fixture": "mixed_reinspection.json",
                    "group": None,
                    "config": yaml,
                    "mode": mode,
                    "only": "Layout/EndOfLine,Style/FrozenStringLiteralComment",
                    "cases": [
                        {
                            "index": index,
                            "name": row["name"],
                            "source": decode(row["source_hex"]),
                            "old_expected": decode(row["after_hex"]),
                        }
                        for index, row in cases
                    ],
                }
            )
    for selected_name, filename, only in [
        (
            "nul_eol",
            "nul_line_endings.json",
            "Layout/SpaceAroundOperators,Layout/EndOfLine,Style/FrozenStringLiteralComment,Layout/SpaceAfterSemicolon",
        ),
        ("nul_writeback", "nul_writeback_cli.json", None),
        ("nul_semicolon", "nul_semicolon_cli.json", "Style/Semicolon"),
    ]:
        if selected_name not in selected:
            continue
        doc = load(filename)
        groups = doc["groups"] if isinstance(doc, dict) else doc
        for index, group in enumerate(groups):
            cops = only or (
                "Layout/SpaceAroundOperators"
                if group["selection"] == "operator"
                else "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile"
            )
            cases = []
            for number, row in enumerate(group["cases"]):
                source = decode(row["source_hex"])
                old_expected = (
                    old_mixed_nul_expected(row)
                    if selected_name == "nul_eol"
                    else native(decode(row["after_hex"]))
                    if b"\r\n" in source
                    else decode(row["after_hex"])
                )
                cases.append(
                    {
                        "index": number,
                        "name": row["name"],
                        "source": source,
                        "old_expected": old_expected,
                    }
                )
            jobs.append(
                {
                    "name": f"{selected_name}-{index:02}",
                    "fixture": filename,
                    "group": index,
                    "config": config(
                        group["ruby"], group["engine"], "lf" if selected_name == "nul_eol" else None
                    ),
                    "mode": None if group["mode"] == "inspect" else group["mode"],
                    "only": cops,
                    "cases": cases,
                }
            )
    for set_name, filename in [
        ("mixed_bare", "mixed_bare_cr.json"),
        ("pure_preservation", "pure_cr_preservation.json"),
    ]:
        if set_name not in selected:
            continue
        doc = load(filename)
        grouped = {}
        for index, row in enumerate(doc["records"]):
            cops = ",".join(row["cops"]) if "cops" in row else doc["only"]
            grouped.setdefault((row["config"], row["mode"], cops), []).append((index, row))
        for number, ((yaml, mode, cops), cases) in enumerate(grouped.items()):
            jobs.append(
                {
                    "name": f"{set_name}-{number:02}",
                    "fixture": filename,
                    "group": None,
                    "config": yaml,
                    "mode": mode,
                    "only": cops,
                    "cases": [
                        {
                            "index": index,
                            "name": row["name"],
                            "source": decode(row["source_hex"]),
                            "old_expected": decode(row["after_hex"]),
                        }
                        for index, row in cases
                    ],
                }
            )
    if "leading" in selected:
        doc = load("leading_empty_lines.json")
        for index, group in enumerate(doc["groups"]):
            cases = []
            for number, name in enumerate(doc["cases"]):
                old_expected = decode(group["first"]["after_hex"][number])
                if group["ending"] == "CRLF":
                    old_expected = native(old_expected)
                cases.append(
                    {
                        "index": number,
                        "name": name,
                        "source": decode(group["source_hex"][number]),
                        "old_expected": old_expected,
                    }
                )
            jobs.append(
                {
                    "name": f"leading-{index:02}",
                    "fixture": "leading_empty_lines.json",
                    "group": index,
                    "config": config(group["ruby"], group["engine"]),
                    "mode": None if group["mode"] == "inspect" else group["mode"],
                    "only": "Layout/LeadingEmptyLines",
                    "cases": cases,
                }
            )

    def invoke(directory, job, mode, files):
        command_args = ["--config", ".rubocop.yml", "--only", job["only"]]
        if mode:
            command_args.append(mode)
        command_args.extend(files)
        result = execute([args.ruby, str(launcher)], directory, command_args, files)
        return {**result, "stderr": ""}

    def sort_key(offense):
        location = offense["location"]
        return (
            location["line"],
            location["column"],
            offense["cop_name"],
            offense["message"],
            offense["severity"],
        )

    def record_job(job):
        directory = out / job["name"]
        directory.mkdir(exist_ok=True)
        (directory / ".rubocop.yml").write_text(job["config"], encoding="utf-8", newline="\n")
        files = [f"input_{index:03}.rb" for index in range(len(job["cases"]))]
        for filename, case in zip(files, job["cases"], strict=True):
            (directory / filename).write_bytes(case["source"])
        initial = invoke(directory, job, job["mode"], files)
        assert "files" in initial["report"], (job["name"], initial)
        upstream_files = initial["report"]["files"]
        afters = [(directory / filename).read_bytes() for filename in files]
        hexes = [case["source"].hex() for case in job["cases"]] + [after.hex() for after in afters]
        (directory / "lex-input.json").write_text(json.dumps(hexes), encoding="utf-8", newline="\n")
        lex = subprocess.run(
            [args.ruby, str(lex_script), "lex-input.json"],
            cwd=directory,
            capture_output=True,
            check=True,
            timeout=60,
            env=oracle_child_env(),
        )
        boundaries = json.loads(lex.stdout)
        count = len(files)
        if len(boundaries) != 2 * count:
            raise ValueError("Prism boundary list is incomplete")
        records = []
        for number, (filename, case, upstream_after, file_report) in enumerate(
            zip(files, job["cases"], afters, upstream_files, strict=True)
        ):
            assert file_report["path"] == filename, (job["name"], filename, file_report["path"])
            source = case["source"]
            written = any(offense["corrected"] for offense in file_report["offenses"])
            before_boundary, after_boundary = boundaries[number], boundaries[count + number]
            if not written:
                physical = source
            elif b"\r\n" in source:
                # 既存の原本 CRLF/provenance 方針を fixture から保つ。
                physical = case["old_expected"]
            elif before_boundary["kind"]:
                assert before_boundary["kind"] == after_boundary["kind"], (
                    job["name"],
                    case["name"],
                    before_boundary,
                    after_boundary,
                )
                if job["fixture"] == "nul_writeback_cli.json":
                    assert b"\r" not in source, (job["name"], case["name"])
                    tail = upstream_after[after_boundary["offset"] :].replace(b"\r\n", b"\n")
                else:
                    tail = source[before_boundary["offset"] :]
                physical = upstream_after[: after_boundary["offset"]] + tail
            else:
                physical = upstream_after
            (directory / filename).write_bytes(physical)
            records.append(
                {
                    "case_index": case["index"],
                    "name": case["name"],
                    "source_sha256": hashlib.sha256(source).hexdigest(),
                    "upstream_after_hex": upstream_after.hex(),
                    "upstream_after_sha256": hashlib.sha256(upstream_after).hexdigest(),
                    "written": written,
                    "original_boundary": before_boundary,
                    "upstream_boundary": after_boundary,
                    "safe_after_hex": physical.hex(),
                    "safe_after_sha256": hashlib.sha256(physical).hexdigest(),
                    "initial_offenses": file_report["offenses"],
                }
            )
        if len(records) != count:
            raise ValueError("reference record list is incomplete")
        reinspection = invoke(directory, job, None, files)
        assert "files" in reinspection["report"], (job["name"], reinspection)
        for filename, record, file_report in zip(
            files, records, reinspection["report"]["files"], strict=True
        ):
            assert file_report["path"] == filename
            assert (directory / filename).read_bytes().hex() == record["safe_after_hex"]
            corrected = [offense for offense in record["initial_offenses"] if offense["corrected"]]
            remaining = file_report["offenses"]
            expected = (
                sorted(corrected + remaining, key=sort_key)
                if record["written"]
                else record["initial_offenses"]
            )
            record["reinspection_offenses"] = remaining
            record["expected_first_offenses"] = expected
            record["expected_repeated_offenses"] = remaining
        first_raw_inputs = [
            record["upstream_after_hex"]
            if record["written"] and not record["original_boundary"]["kind"]
            else record["safe_after_hex"]
            for record in records
        ]
        if any(
            raw != record["safe_after_hex"]
            for raw, record in zip(first_raw_inputs, records, strict=True)
        ):
            for filename, raw in zip(files, first_raw_inputs, strict=True):
                (directory / filename).write_bytes(bytes.fromhex(raw))
            first_raw_reinspection = invoke(directory, job, None, files)
        else:
            first_raw_reinspection = reinspection
        for filename, raw, record, file_report in zip(
            files, first_raw_inputs, records, first_raw_reinspection["report"]["files"], strict=True
        ):
            corrected = [offense for offense in record["initial_offenses"] if offense["corrected"]]
            record["first_raw_input_hex"] = raw
            record["first_raw_offenses"] = file_report["offenses"]
            record["expected_first_offenses"] = (
                sorted(corrected + file_report["offenses"], key=sort_key)
                if record["written"]
                else record["initial_offenses"]
            )
            (directory / filename).write_bytes(bytes.fromhex(record["safe_after_hex"]))
        result = {key: value for key, value in job.items() if key != "cases"}
        result.update(
            {
                "initial": initial,
                "reinspection": reinspection,
                "first_raw_reinspection": first_raw_reinspection,
                "records": records,
            }
        )
        (out / (job["name"] + ".json")).write_text(
            json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
        )
        print(job["name"], len(records), initial["exit"], reinspection["exit"], flush=True)
        return {
            "name": job["name"],
            "fixture": job["fixture"],
            "group": job["group"],
            "cases": len(records),
            "initial_exit": initial["exit"],
            "reinspection_exit": reinspection["exit"],
        }

    summaries = []
    errors = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = {pool.submit(record_job, job): job["name"] for job in jobs}
        for future in concurrent.futures.as_completed(futures):
            try:
                summaries.append(future.result())
            except Exception as error:
                errors.append({"job": futures[future], "error": repr(error)})
                print("ERROR", futures[future], repr(error), flush=True)
            (out / "summary.json").write_text(
                json.dumps(
                    {"completed": summaries, "errors": errors}, ensure_ascii=False, indent=2
                ),
                encoding="utf-8",
                newline="\n",
            )
    if errors:
        raise SystemExit(1)

    if len(summaries) != len(jobs) or not jobs:
        raise ValueError("reference jobs did not finish")
    artifact = assemble(out, jobs, fixtures, versions)
    if set(artifact["cases"]) != {SET_FIXTURES[name] for name in selected}:
        raise ValueError("requested fixture sets are incomplete")
    if args.check_against:
        expected = json.loads(args.check_against.read_text(encoding="utf-8"))
        for name, cases in artifact["cases"].items():
            if cases != expected["cases"][name]:
                raise ValueError(f"regenerated cases differ from the recorded Oracle: {name}")
    (out / "windows_native_cli_expectations.json").write_text(
        json.dumps(artifact, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print({name: len(cases) for name, cases in artifact["cases"].items()})


if __name__ == "__main__":
    main()
