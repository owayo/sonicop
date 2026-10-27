use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::engine::native_textmode_bytes;

/// Ruby IO が受け取る logical な write 引数。puts の追加 LF も別の引数として保つ。
pub(crate) trait Output {
    fn write(&mut self, text: &str) -> Result<()>;

    fn owned(&mut self, text: String) -> Result<()> {
        self.write(&text)
    }

    fn puts(&mut self, text: &str) -> Result<()> {
        self.write(text)?;
        if text.is_empty() || !text.ends_with('\n') {
            self.write("\n")?;
        }
        Ok(())
    }
}

impl Output for String {
    fn write(&mut self, text: &str) -> Result<()> {
        self.push_str(text);
        Ok(())
    }

    fn owned(&mut self, text: String) -> Result<()> {
        if self.is_empty() {
            // JSON/HTML の全体文書は finished の一引数。公開renderで二度複写しない。
            *self = text;
        } else {
            self.push_str(&text);
        }
        Ok(())
    }
}

const FILE_BUFFER_SIZE: usize = 8192;

/// 同じ path でも Ruby は各 formatter の File offset と buffer を共有しない。
/// 8192 の判定は text mode より前の bytes。UTF-8 文字数や物理 CRLF 長で測らない。
struct LogicalFile {
    file: File,
    path: PathBuf,
    pending: Vec<u8>,
    failed: bool,
}

impl LogicalFile {
    fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let file =
            File::create(path).with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Self {
            file,
            path: path.to_owned(),
            pending: Vec::new(),
            failed: false,
        })
    }

    fn physical_write(&mut self, bytes: &[u8]) -> Result<()> {
        let result = self.file.write_all(&native_textmode_bytes(bytes));
        self.failed |= result.is_err();
        result.with_context(|| format!("failed to write {}", self.path.display()))
    }

    fn flush_pending(&mut self) -> Result<()> {
        if self.pending.is_empty() || self.failed {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        let result = self.physical_write(&pending);
        self.pending = pending;
        self.pending.clear();
        result
    }

    fn write(&mut self, text: &str) -> Result<()> {
        let incoming = text.as_bytes();
        if incoming.is_empty() {
            return Ok(());
        }
        let combined = self.pending.len().saturating_add(incoming.len());
        if combined >= FILE_BUFFER_SIZE {
            // Ruby io_binwrite は等号でも flush し、溢れた今回の引数は直接 write する。
            // flush 後に今回分を再bufferする BufWriter の境界とは異なる。
            if !self.pending.is_empty() && combined == FILE_BUFFER_SIZE {
                self.pending.extend_from_slice(incoming);
                self.flush_pending()
            } else {
                self.flush_pending()?;
                self.physical_write(incoming)
            }
        } else {
            if self.pending.capacity() == 0 {
                self.pending.reserve_exact(FILE_BUFFER_SIZE);
            }
            self.pending.extend_from_slice(incoming);
            Ok(())
        }
    }
}

enum Target {
    Stream,
    File(LogicalFile),
}

/// File は全 started の前に開く。同じ path/alias をひとつの handle へ畳まない。
pub(crate) struct OutputSet {
    targets: Vec<Target>,
    stdout: BufWriter<io::Stdout>,
    stderr: BufWriter<io::Stderr>,
    use_stderr: bool,
}

impl OutputSet {
    pub(crate) fn open(
        paths: &[Option<PathBuf>],
        use_stderr: bool,
        mut initialize: impl FnMut(usize) -> Result<()>,
    ) -> Result<Self> {
        let mut outputs = Self {
            targets: Vec::with_capacity(paths.len()),
            stdout: BufWriter::new(io::stdout()),
            stderr: BufWriter::new(io::stderr()),
            use_stderr,
        };
        for (index, path) in paths.iter().enumerate() {
            outputs.targets.push(match path {
                Some(path) => Target::File(LogicalFile::open(path)?),
                None => Target::Stream,
            });
            initialize(index)?;
        }
        Ok(outputs)
    }

    pub(crate) fn writer(&mut self, index: usize) -> impl Output + '_ {
        TargetOutput {
            outputs: self,
            index,
        }
    }

    pub(crate) fn finish(&mut self) -> Result<()> {
        for target in &mut self.targets {
            if let Target::File(file) = target {
                file.flush_pending()?;
            }
        }
        self.stdout.flush()?;
        self.stderr.flush()?;
        Ok(())
    }
}

impl Drop for OutputSet {
    fn drop(&mut self) {
        // 描画途中の error でも、別 File の残りを formatter 登録順にclose相当へ進める。
        // write_all が一部を書いて失敗した handle は再試行してprefixを二重にしない。
        for target in &mut self.targets {
            if let Target::File(file) = target {
                let _ = file.flush_pending();
            }
        }
    }
}

struct TargetOutput<'a> {
    outputs: &'a mut OutputSet,
    index: usize,
}

impl Output for TargetOutput<'_> {
    fn write(&mut self, text: &str) -> Result<()> {
        match &mut self.outputs.targets[self.index] {
            Target::File(file) => file.write(text),
            Target::Stream => {
                let bytes = native_textmode_bytes(text.as_bytes());
                if self.outputs.use_stderr {
                    self.outputs.stderr.write_all(&bytes)?;
                } else {
                    self.outputs.stdout.write_all(&bytes)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::OutputSet;
    use super::{LogicalFile, Output};
    use std::fs::{self, File};

    #[test]
    fn complete_document_keeps_its_allocation_in_the_string_sink() {
        let document = format!("{{\"value\":\"{}\"}}", "x".repeat(9000));
        let pointer = document.as_ptr();
        let mut output = String::new();
        output.owned(document).unwrap();
        assert_eq!(output.as_ptr(), pointer);
        assert!(output.len() > 8192);
    }

    #[test]
    fn a_buffered_os_write_error_is_returned() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("readonly-handle.txt");
        fs::write(&path, b"original").unwrap();
        // 閉じたhandleを偽装せず、実OSの読取専用handleへのwriteを失敗させる。
        let mut output = LogicalFile {
            file: File::open(&path).unwrap(),
            path: path.clone(),
            pending: Vec::new(),
            failed: false,
        };
        output.write("new\n").unwrap();
        assert!(output.flush_pending().is_err());
        assert_eq!(fs::read(path).unwrap(), b"original");
    }

    #[cfg(windows)]
    #[test]
    fn independent_native_write_arguments_preserve_physical_file_bytes() {
        fn decode(value: &serde_json::Value) -> Vec<u8> {
            value
                .as_str()
                .unwrap()
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/formatter_lifecycle.json"
        ))
        .unwrap();
        let cases = fixture["buffer_cases"].as_array().unwrap();
        assert_eq!(cases.len(), 21);
        for case in cases {
            let folder = tempfile::tempdir().unwrap();
            let count = case["targets"].as_array().unwrap().len();
            let separate = case["separate_outputs"].as_bool().unwrap();
            let paths = (0..count)
                .map(|index| {
                    Some(folder.path().join(if separate {
                        format!("out{index}.txt")
                    } else {
                        "shared.txt".to_owned()
                    }))
                })
                .collect::<Vec<_>>();
            let mut outputs = OutputSet::open(&paths, false, |_| Ok(())).unwrap();
            for write in case["writes"].as_array().unwrap() {
                let index = write["target"].as_u64().unwrap() as usize;
                for argument in write["segments_hex"].as_array().unwrap() {
                    let argument = String::from_utf8(decode(argument)).unwrap();
                    outputs.writer(index).write(&argument).unwrap();
                }
            }
            outputs.finish().unwrap();
            for (index, expected) in case["expected_files_hex"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_eq!(
                    fs::read(paths[index].as_ref().unwrap()).unwrap(),
                    decode(expected),
                    "{}",
                    case["case"]
                );
            }
        }
    }
}
