use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;

const BLOCK_BYTES: u64 = 128 * 1024;
const MAX_ROW_BYTES: usize = 2048;

pub struct TextPreviewReader {
    file: File,
    len: u64,
}

pub struct TextPreviewWindow {
    pub bytes: Range<u64>,
    pub rows: Vec<(u64, String)>,
}

impl TextPreviewReader {
    pub fn open(path: &Path, len: u64) -> io::Result<Self> {
        if !path.metadata()?.is_file() {
            return Err(io::Error::other("Quick View requires a regular file"));
        }
        Ok(Self {
            file: File::open(path)?,
            len,
        })
    }

    pub fn block(offset: u64) -> u64 {
        offset / BLOCK_BYTES
    }

    pub fn read_window(&mut self, block: u64) -> io::Result<TextPreviewWindow> {
        if block > Self::block(self.len.saturating_sub(1)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Text preview offset is past EOF",
            ));
        }
        let first = block.saturating_sub(1);
        let last = block + 2;
        let base = first * BLOCK_BYTES;
        let end = (last * BLOCK_BYTES + MAX_ROW_BYTES as u64 + 4).min(self.len);
        let mut bytes = vec![0; end.saturating_sub(base) as usize];
        self.file.seek(SeekFrom::Start(base))?;
        self.file.read_exact(&mut bytes)?;

        // Neighboring windows use the same block boundaries so their rows agree.
        let mut start = if first == 0 { 0 } else { boundary(&bytes, 0) };
        let mut window = TextPreviewWindow {
            bytes: base + start as u64..base + start as u64,
            rows: Vec::new(),
        };
        for block in first + 1..=last {
            let nominal = ((block - first) * BLOCK_BYTES) as usize;
            let end = boundary(&bytes, nominal);
            for line in bytes[start..end].split_inclusive(|&byte| byte == b'\n') {
                let text = line
                    .strip_suffix(b"\n")
                    .map_or(line, |line| line.strip_suffix(b"\r").unwrap_or(line));
                let mut offset = 0;
                loop {
                    let cut = utf8_boundary(text, offset + MAX_ROW_BYTES);
                    window.rows.push((
                        base + (start + offset) as u64,
                        String::from_utf8_lossy(&text[offset..cut]).into_owned(),
                    ));
                    if cut == text.len() {
                        break;
                    }
                    offset = cut;
                }
                start += line.len();
            }
            if end == bytes.len() {
                break;
            }
        }
        window.bytes.end = base + start as u64;
        Ok(window)
    }
}

fn boundary(bytes: &[u8], nominal: usize) -> usize {
    if nominal >= bytes.len() {
        return bytes.len();
    }
    let scan_end = (nominal + MAX_ROW_BYTES).min(bytes.len());
    bytes[nominal..scan_end]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or_else(
            || utf8_boundary(bytes, nominal),
            |index| nominal + index + 1,
        )
}

fn utf8_boundary(bytes: &[u8], offset: usize) -> usize {
    let mut offset = offset.min(bytes.len());
    // A valid UTF-8 sequence has at most three continuation bytes.
    for _ in 0..3 {
        if bytes.get(offset).is_none_or(|byte| byte & 0xc0 != 0x80) {
            break;
        }
        offset += 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn consecutive_windows_agree_across_newlines_and_long_utf8_rows() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let first = "ordinary line\r\n".repeat(10_000);
        let long = "€".repeat(BLOCK_BYTES as usize);
        write!(file, "{first}{long}\r\nlast line").unwrap();
        let len = file.as_file().metadata().unwrap().len();
        let mut reader = TextPreviewReader::open(file.path(), len).unwrap();
        let start = reader.read_window(0).unwrap();
        let middle = reader.read_window(2).unwrap();
        let end = reader
            .read_window(TextPreviewReader::block(len - 1))
            .unwrap();
        assert_eq!(start.rows[0], (0, "ordinary line".to_owned()));
        assert_eq!(end.rows.last().unwrap(), &(len - 9, "last line".to_owned()));
        assert_eq!(end.bytes.end, len);
        for (left, right) in [(&start, &middle), (&middle, &end)] {
            let overlap =
                left.bytes.start.max(right.bytes.start)..left.bytes.end.min(right.bytes.end);
            assert!(!overlap.is_empty());
            let in_overlap = |window: &TextPreviewWindow| {
                window
                    .rows
                    .iter()
                    .filter(|(offset, _)| overlap.contains(offset))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(in_overlap(left), in_overlap(right));
        }
        assert!(middle.rows.iter().all(|(_, text)| !text.contains('�')));
    }

    #[cfg(unix)]
    #[test]
    fn seeks_past_four_gibibytes_with_a_bounded_read() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let marker = 10_000_000_000;
        file.as_file_mut()
            .seek(SeekFrom::Start(marker - 1))
            .unwrap();
        file.write_all(b"\nLast record").unwrap();
        let len = file.as_file().metadata().unwrap().len();
        let mut reader = TextPreviewReader::open(file.path(), len).unwrap();
        let window = reader
            .read_window(TextPreviewReader::block(len - 1))
            .unwrap();
        assert!(window.bytes.end - window.bytes.start <= 3 * BLOCK_BYTES + MAX_ROW_BYTES as u64);
        assert_eq!(
            window.rows.last().unwrap(),
            &(marker, "Last record".to_owned())
        );
    }
}
