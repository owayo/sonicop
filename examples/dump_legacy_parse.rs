//! TargetRubyVersion 2.7〜3.4 の構文解析 (parser gem の移植) が出したトークンと診断を書き出す。
//! parser gem の `Parser::RubyXX#tokenize` と突き合わせるための道具。
//!
//! ```bash
//! cargo run --example dump_legacy_parse -- 2.7 path/to/source.rb [more.rb ...]
//! ```
//!
//! トークンは `型 開始 終了`、診断は `水準 開始-終了 文言` (位置は CRLF を LF にした本文の文字単位)。
//! ファイルを複数渡すと、それぞれの前に `=== パス` を書く。

use std::io::{BufWriter, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((target, paths)) = args.split_first().filter(|(_, paths)| !paths.is_empty()) else {
        eprintln!("usage: dump_legacy_parse <2.7|3.0|3.1|3.2|3.3|3.4> <file>...");
        return ExitCode::from(2);
    };
    let Some(target) = sonicop::RubyVersion::parse(target) else {
        eprintln!("unknown version: {target}");
        return ExitCode::from(2);
    };
    let many = paths.len() > 1;
    // 巨大なコーパスの比較で、字句ごとの write syscall を解析コストへ混ぜない。
    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    for path in paths {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("{path}: {error}");
                continue;
            }
        };
        let Ok(text) = String::from_utf8(bytes) else {
            // parser gem の Buffer は不正な UTF-8 を拒むので、比べる対象にしない。
            continue;
        };
        if many {
            writeln!(output, "=== {path}").unwrap();
        }
        let Some((tokens, diagnostics)) = sonicop::legacy_parser::debug_tokens(&text, target)
        else {
            eprintln!("not a parser gem target: {target}");
            return ExitCode::from(2);
        };
        for (name, start, end) in tokens {
            writeln!(output, "{name} {start} {end}").unwrap();
        }
        for diagnostic in diagnostics {
            writeln!(output, "{diagnostic}").unwrap();
        }
    }
    output.flush().unwrap();
    ExitCode::SUCCESS
}
