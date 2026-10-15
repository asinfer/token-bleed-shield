//! Incremental SSE frame scanner (docs/architecture/02 §2.2).
//!
//! Zero-allocation steady state, carries partial frames across chunk boundaries,
//! caps single-event accumulation at 256 KiB, and never retains payload beyond
//! the current frame.

pub const DEFAULT_FRAME_CAP_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

pub struct SseScanner {
    buf: Vec<u8>,
    cap: usize,
    overflowed: bool,
}

impl SseScanner {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap,
            overflowed: false,
        }
    }

    /// Feed a chunk; returns every complete frame terminated by a blank line.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buf.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(pos) = find_frame_boundary(&self.buf) {
            let frame_bytes: Vec<u8> = self.buf.drain(..pos + 1).collect();
            if let Some(frame) = parse_frame(&frame_bytes) {
                frames.push(frame);
            }
        }
        if self.buf.len() > self.cap {
            self.buf.clear();
            self.overflowed = true;
        }
        frames
    }

    /// Reports (and clears) whether a single-event cap was exceeded since last call.
    pub fn take_overflow(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }
}

fn find_frame_boundary(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i + 1);
        }
        if i + 3 < buf.len()
            && buf[i] == b'\r'
            && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r'
            && buf[i + 3] == b'\n'
        {
            return Some(i + 3);
        }
        i += 1;
    }
    None
}

fn parse_frame(frame_bytes: &[u8]) -> Option<SseFrame> {
    let text = String::from_utf8_lossy(frame_bytes);
    let mut event: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if event.is_none() && data_lines.is_empty() {
        return None;
    }
    Some(SseFrame {
        event,
        data: data_lines.join("\n"),
    })
}

/// Bounded tail buffer for sniffing usage fields in non-SSE JSON responses.
pub struct TailBuffer {
    buf: Vec<u8>,
    cap: usize,
}

impl TailBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let excess = self.buf.len() - self.cap;
            self.buf.drain(..excess);
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_across_arbitrary_chunk_boundaries() {
        let payload = "data: {\"a\":1}\n\ndata: {\"b\":2}\n\n";
        for split in 1..payload.len() {
            let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
            let mut frames = scanner.feed(&payload.as_bytes()[..split]);
            frames.extend(scanner.feed(&payload.as_bytes()[split..]));
            assert_eq!(frames.len(), 2, "split at {split}");
            assert_eq!(frames[0].data, "{\"a\":1}");
            assert_eq!(frames[1].data, "{\"b\":2}");
        }
    }

    #[test]
    fn byte_at_a_time_feed() {
        let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
        let mut frames = Vec::new();
        for b in b"event: content_block_delta\ndata: {\"x\":1}\n\n" {
            frames.extend(scanner.feed(&[*b]));
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(frames[0].data, "{\"x\":1}");
    }

    #[test]
    fn crlf_boundaries() {
        let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
        let frames = scanner.feed(b"data: a\r\n\r\ndata: b\r\n\r\n");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "a");
        assert_eq!(frames[1].data, "b");
    }

    #[test]
    fn multiline_data_is_joined() {
        let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
        let frames = scanner.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(frames[0].data, "line1\nline2");
    }

    #[test]
    fn comments_ignored_and_empty_frames_skipped() {
        let mut scanner = SseScanner::new(DEFAULT_FRAME_CAP_BYTES);
        let frames = scanner.feed(b": keepalive\n\n\ndata: x\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "x");
    }

    #[test]
    fn overflow_caps_unterminated_event() {
        let mut scanner = SseScanner::new(64);
        let frames = scanner.feed(&[b'x'; 128]);
        assert!(frames.is_empty());
        assert!(scanner.take_overflow());
        assert!(!scanner.take_overflow());
    }

    #[test]
    fn tail_buffer_keeps_last_bytes_only() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"0123456789");
        assert_eq!(tail.text(), "23456789");
    }
}
