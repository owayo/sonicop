"""本家CLIだけを実行し、未知/曖昧copの段階・空report・原本不変を記録する。"""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

ENV_OPTIONS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
LAUNCHER = r"""
gem 'rubocop', '=1.89.0'
gem 'parser', '=3.3.12.0'
gem 'prism', '=1.8.1'
gem 'rainbow', '=3.1.1'
require 'rubocop'
require 'json'
metadata = {ruby: RUBY_VERSION, parser: Parser::VERSION, prism: Prism::VERSION,
            rainbow: Gem.loaded_specs.fetch('rainbow').version.to_s}
File.binwrite(ARGV.shift, JSON.generate(metadata))
exit RuboCop::CLI.new.run(ARGV)
"""

# 期待値は含めず、有限の入力条件だけを保存する。実行結果は毎回本家から取得する。
SCENARIOS = json.loads(r"""[
  {
    "id": "only-unknown-disk",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-stdin",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--stdin",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-zero",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "empty"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-two",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "input.rb",
      "other.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-list-unknown",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--list-target-files",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-ignore-unknown",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--ignore-unrecognized-cops",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-unknown-disk",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-unknown-stdin",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "--stdin",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-unknown-zero",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "empty"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-unknown-two",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "input.rb",
      "other.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-list-unknown",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "--list-target-files",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "except-ignore-unknown",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--except",
      "Unknown/Cop",
      "--ignore-unrecognized-cops",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-safe",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--safe",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-unsafe",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "-A",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-stdin-safe",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "-a",
      "--stdin",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-lint",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--lint",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-layout",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--fix-layout",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-fail-fast",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--fail-fast",
      "input.rb",
      "other.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-missing",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "missing.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-wrong-namespace",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop,Wrong/LineLength",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-except-wrong",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--except",
      "Wrong/LineLength",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-wrong-except-unknown",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Wrong/LineLength",
      "--except",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-ambiguous",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "MultipleComparison",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-ambiguous-zero",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "MultipleComparison",
      "empty"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-ambiguous-wrong",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "MultipleComparison,Wrong/LineLength",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-wrong-ambiguous",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Wrong/LineLength,MultipleComparison",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-wrong-valid",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Wrong/LineLength",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-short-valid",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "LineLength",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-progress",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--format",
      "progress",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-dual-out",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--format",
      "progress",
      "--out",
      "progress.txt",
      "--format",
      "json",
      "--out",
      "result.json",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-stderr",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--stderr",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-near-name",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Layout/SpaceAroundOperator",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-wrong-ambiguity",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Wrong/MultipleComparison",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-invalid-encoding",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "ff0d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-invalid-syntax",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "646566206e280d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-progress-alone",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "progress",
      "--only",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "only-unknown-warning-stderr",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "progress",
      "--only",
      "Unknown/Cop,Wrong/LineLength",
      "--stderr",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "untraced-unknown-disk",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "untraced-unknown-warning-stderr",
    "args": [
      "--config",
      ".rubocop.yml",
      "--cache",
      "false",
      "--format",
      "progress",
      "--only",
      "Unknown/Cop,Wrong/LineLength",
      "--stderr",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "single-cachetrue-disk",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "true",
      "--cache-root",
      "cache",
      "--no-parallel",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "single-cachetrue-missing",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "true",
      "--cache-root",
      "cache",
      "--no-parallel",
      "missing.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "missing-after-existing",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "false",
      "input.rb",
      "missing.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "missing-before-existing",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "false",
      "missing.rb",
      "input.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "missing-unrecognized-extension",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "false",
      "--only-recognized-file-types",
      "missing.txt"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "missing-recognized-extension",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "false",
      "--only-recognized-file-types",
      "missing.rb"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  },
  {
    "id": "missing-force-excluded",
    "args": [
      "--config",
      ".rubocop.yml",
      "--format",
      "json",
      "--only",
      "Unknown/Cop",
      "--cache",
      "false",
      "--force-exclusion",
      "missing.txt"
    ],
    "inputs": {
      "input.rb": "783d310d0a",
      "other.rb": "793d320d0a",
      ".rubocop.yml": "416c6c436f70733a0a20204e6577436f70733a2064697361626c650a20205461726765745275627956657273696f6e3a20322e370a"
    },
    "stdin_hex": "783d310d0a"
  }
]""")

# Registryは曖昧名をBadge.parse後の表記で報告する。正規名を対照として一緒に採取する。
for case_id, name in (
    ("badge-canonical-ambiguity", "MultipleComparison"),
    ("badge-snake-ambiguity", "multiple_comparison"),
    ("badge-lowercase-ambiguity", "multipleComparison"),
    ("badge-namespace-ambiguity", "wrong/MultipleComparison"),
):
    SCENARIOS.append(
        {
            "id": case_id,
            "args": [
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--format",
                "json",
                "--only",
                name,
                "input.rb",
            ],
            "inputs": SCENARIOS[0]["inputs"].copy(),
            "stdin_hex": SCENARIOS[0]["stdin_hex"],
        }
    )


DIRECTORY_SCENARIOS = [
    {
        "id": name,
        "args": [
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--format",
            "json",
            "--only",
            "Unknown/Cop,Wrong/LineLength",
            *extra,
            "--stdin",
            "directory",
        ],
        "inputs": {
            "config.yml": b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n".hex(),
            **{path: source.encode().hex() for path, source in inputs.items()},
        },
        "directories": ["directory"],
        "stdin_hex": b"x=1\r\n".hex(),
    }
    for name, inputs, extra in (
        ("one", {"directory/input.rb": "physical = true\n"}, []),
        (
            "two",
            {"directory/a.rb": "physical = true\n", "directory/b.rb": "physical = false\n"},
            [],
        ),
        ("empty", {}, []),
        ("listing", {"directory/input.rb": "physical = true\n"}, ["--list-target-files"]),
        (
            "safe-progress",
            {"directory/input.rb": "physical = true\n"},
            ["-a", "--format", "progress"],
        ),
        ("empty-safe-progress", {}, ["-a", "--format", "progress"]),
    )
]


def _environment():
    environment = os.environ.copy()
    for key in ENV_OPTIONS:
        environment.pop(key, None)
    return environment


def _relocate(raw, cwd):
    return raw.replace(str(cwd).replace("\\", "/").encode(), b"${CWD}").replace(
        str(cwd).encode(), b"${CWD}"
    )


def _record(ruby, scenario):
    with tempfile.TemporaryDirectory(prefix="sonicop-unknown-cop-") as temporary:
        container = Path(temporary)
        cwd = container / "project"
        cwd.mkdir()
        (cwd / "empty").mkdir()
        for name in scenario.get("directories", []):
            (cwd / name).mkdir(parents=True, exist_ok=True)
        for name, value in scenario["inputs"].items():
            (cwd / name).parent.mkdir(parents=True, exist_ok=True)
            (cwd / name).write_bytes(bytes.fromhex(value))
        result = subprocess.run(
            [ruby, "-e", LAUNCHER, str(container / "runtime.json"), *scenario["args"]],
            cwd=cwd,
            env=_environment(),
            input=bytes.fromhex(scenario["stdin_hex"]),
            capture_output=True,
            check=False,
            timeout=120,
        )
        runtime = json.loads((container / "runtime.json").read_bytes())
        for name, value in scenario["inputs"].items():
            if (cwd / name).read_bytes() != bytes.fromhex(value):
                raise ValueError(f"原本が変わった: {scenario['id']} / {name}")
        outputs = {
            path.name: _relocate(path.read_bytes(), cwd).hex()
            for path in sorted(cwd.iterdir())
            if path.is_file() and path.name not in scenario["inputs"]
        }
        reference = {
            "exit": result.returncode,
            "stdout_hex": _relocate(result.stdout, cwd).hex(),
            "stderr_hex": _relocate(result.stderr, cwd).hex(),
            "outputs": outputs,
        }
        return {**scenario, "reference": reference}, runtime


def record(ruby, scenarios=SCENARIOS):
    records = [_record(ruby, scenario) for scenario in scenarios]
    runtimes = [runtime for _, runtime in records]
    expected = {"ruby": "4.0.7", "parser": "3.3.12.0", "prism": "1.8.1", "rainbow": "3.1.1"}
    if any(runtime != expected for runtime in runtimes):
        raise ValueError(f"参照版が違う: {runtimes}")
    return {
        "specification": "RuboCop 1.89.0",
        "reference_runtime": expected,
        "cwd_placeholder": "${CWD}",
        "cases": [case for case, _ in records],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--set", choices=("core", "directory"), default="core")
    parser.add_argument(
        "--output", type=Path, default=Path("tests/fixtures/unknown_cop_report.json")
    )
    options = parser.parse_args()
    options.output.write_text(
        json.dumps(
            record(options.ruby, DIRECTORY_SCENARIOS if options.set == "directory" else SCENARIOS),
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
        newline="\n",
    )


if __name__ == "__main__":
    main()
