//! Minimal incremental Server-Sent Events parser: events may arrive split
//! across any number of network chunks; each is bounded in size.

pub(crate) struct SseParser {
    buf: Vec<u8>,
    max_event_bytes: usize,
}

impl SseParser {
    pub(crate) fn new(max_event_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_event_bytes,
        }
    }

    /// Feeds bytes and returns the `data:` payload of every event completed
    /// by them (multi-line data joined with `\n`).
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.buf.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, separator)) = boundary(&self.buf) {
            let raw: Vec<u8> = self.buf.drain(..end + separator).collect();
            if let Some(data) = data_of(&raw[..end])? {
                events.push(data);
            }
        }
        if self.buf.len() > self.max_event_bytes {
            return Err(format!("an event exceeded {} bytes", self.max_event_bytes));
        }
        Ok(events)
    }

    /// A last event the stream ended without terminating with a blank line.
    pub(crate) fn finish(&mut self) -> Result<Option<String>, String> {
        let rest = std::mem::take(&mut self.buf);
        data_of(&rest)
    }
}

fn boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find(buf, b"\n\n").map(|i| (i, 2));
    let crlf = find(buf, b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn data_of(raw: &[u8]) -> Result<Option<String>, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "event is not UTF-8".to_string())?;
    let lines: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|d| d.strip_prefix(' ').unwrap_or(d))
        .collect();
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn events_split_across_chunks_are_reassembled() {
        let mut p = SseParser::new(1024);
        assert!(p.push(b"data: {\"a\"").unwrap().is_empty());
        assert_eq!(
            p.push(b":1}\n\ndata: x\r\n\r\n").unwrap(),
            vec!["{\"a\":1}", "x"]
        );
    }

    #[test]
    fn comments_and_other_fields_are_ignored() {
        let mut p = SseParser::new(1024);
        assert_eq!(
            p.push(b": keep-alive\n\nevent: msg\ndata: y\n\n").unwrap(),
            vec!["y"]
        );
    }

    #[test]
    fn oversized_event_is_refused() {
        let mut p = SseParser::new(8);
        assert!(p.push(b"data: 0123456789").is_err());
    }

    #[test]
    fn unterminated_last_event_is_recovered_at_end() {
        let mut p = SseParser::new(1024);
        assert!(p.push(b"data: tail").unwrap().is_empty());
        assert_eq!(p.finish().unwrap().as_deref(), Some("tail"));
    }
}
