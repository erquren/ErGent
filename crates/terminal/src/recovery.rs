//! Bounded raw-byte replay. The screen always consumes every PTY byte first.
use std::collections::VecDeque;

pub(crate) const REPLAY_BYTES: usize = 512 * 1024;
pub(crate) const CHUNK_BYTES: usize = 16 * 1024;
pub(crate) const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct Recovery {
    bytes: VecDeque<u8>,
    pub seq: u64,
    pub revision: u64,
    probe: Option<Box<vte::Parser>>,
    ground: bool,
    // A snapshot must precede an incomplete ANSI/UTF-8 sequence, never cut it.
    checkpoint: Option<(u64, Vec<u8>)>,
}
impl Recovery {
    pub fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            seq: 0,
            revision: 0,
            probe: Some(Box::new(vte::Parser::new())),
            ground: true,
            checkpoint: None,
        }
    }
    pub fn process(&mut self, parser: &mut vt100::Parser, bytes: &[u8]) -> Vec<u8> {
        let was_ground = self.ground;
        let mut boundary = None;
        let mut queries = Vec::new();
        let mut filtered = bytes.to_vec();
        for (index, &byte) in bytes.iter().enumerate() {
            let mut probe = Boundary {
                ground: self.ground && (byte < 0x20 || byte == 0x7f) && byte != 0x1b,
                query: None,
            };
            self.probe
                .as_mut()
                .expect("live parser probe")
                .advance(&mut probe, byte);
            self.ground = probe.ground;
            if let Some(query) = probe.query {
                // CAN terminates this CSI without invoking browser replies. A
                // one-byte replacement preserves all replay byte offsets, even
                // when the query prefix was delivered in an earlier chunk.
                filtered[index] = 0x18;
                queries.push((index, query));
            }
            if self.ground {
                boundary = Some(index + 1);
            }
        }
        let mut replies = Vec::new();
        let mut start = 0;
        for (index, query) in queries {
            parser.process(&filtered[start..=index]);
            replies.extend(query.reply(parser.screen()));
            start = index + 1;
        }
        let bytes = filtered.as_slice();
        if let Some(end) = boundary {
            parser.process(&bytes[start..end]);
            self.checkpoint = (end != bytes.len()).then(|| {
                (
                    self.seq + end as u64,
                    super::screen::snapshot(parser.screen()),
                )
            });
            parser.process(&bytes[end..]);
        } else {
            if was_ground {
                self.checkpoint = Some((self.seq, super::screen::snapshot(parser.screen())));
            }
            parser.process(bytes);
        }
        self.seq += bytes.len() as u64;
        self.bytes.extend(bytes);
        if self.bytes.len() > REPLAY_BYTES {
            self.bytes.drain(..self.bytes.len() - REPLAY_BYTES);
        }
        replies
    }
    pub fn contains(&self, seq: u64) -> bool {
        (self.seq - self.bytes.len() as u64..=self.seq).contains(&seq)
    }
    pub fn read(&self, seq: u64) -> Option<Vec<u8>> {
        if !self.contains(seq) {
            return None;
        }
        let start = (seq - (self.seq - self.bytes.len() as u64)) as usize;
        Some(
            self.bytes
                .range(start..(start + CHUNK_BYTES).min(self.bytes.len()))
                .copied()
                .collect(),
        )
    }
    pub fn snapshot(&self, parser: &vt100::Parser) -> Option<(u64, Vec<u8>)> {
        let snapshot = if self.ground {
            (self.seq, super::screen::snapshot(parser.screen()))
        } else {
            self.checkpoint.clone()?
        };
        // Never forge a cursor if an unterminated sequence outgrew the ring.
        (self.contains(snapshot.0) && snapshot.1.len() <= MAX_SNAPSHOT_BYTES).then_some(snapshot)
    }
    pub fn clear(&mut self) {
        self.bytes = VecDeque::new();
        self.checkpoint = None;
        self.probe = None;
    }
    pub fn resized(&mut self) {
        self.revision += 1;
        self.bytes.clear();
        self.checkpoint = None;
    }
}

/// Conservatively identify complete parser boundaries with the *same* VTE parser
/// as vt100. C0 controls inside CSI/OSC do not finish that sequence. OSC/DCS
/// terminated by ESC are still inside an escape until its final byte arrives.
struct Boundary {
    ground: bool,
    query: Option<super::query::Query>,
}
impl vte::Perform for Boundary {
    fn print(&mut self, _: char) {
        self.ground = true;
    }
    fn execute(&mut self, byte: u8) {
        if byte == 0x18 || byte == 0x1a {
            self.ground = true;
        }
    }
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        self.ground = true;
        self.query = super::query::Query::parse(params, intermediates, ignore, action);
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
        self.ground = true;
    }
    fn osc_dispatch(&mut self, _: &[&[u8]], bell: bool) {
        self.ground = bell;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recover(chunks: &[&[u8]]) {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        for chunk in chunks {
            recovery.process(&mut parser, chunk);
        }
        let (seq, snapshot) = recovery.snapshot(&parser).unwrap();
        let mut restored = vt100::Parser::new(24, 80, 0);
        restored.process(&snapshot);
        restored.process(&recovery.read(seq).unwrap());
        let suffix = b"m after\x1b[0m\r\nnext\x1b[?1049l";
        recovery.process(&mut parser, suffix);
        restored.process(suffix);
        assert_eq!(restored.screen().contents(), parser.screen().contents());
        assert_eq!(
            restored.screen().cursor_position(),
            parser.screen().cursor_position()
        );
    }
    #[test]
    fn live_queries_reply_once_at_their_actual_cursor_positions() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        let bytes = b"\x1b[2;3H\x1b[6n\x1b[5;8H\x1b[6n\x1b[5n\x1b[c\x1b[>c";
        let expected = b"\x1b[2;3R\x1b[5;8R\x1b[0n\x1b[?1;2c\x1b[>0;0;0c";
        assert_eq!(recovery.process(&mut parser, bytes), expected);
        assert_eq!(recovery.seq, bytes.len() as u64);
        let replay = recovery.read(0).unwrap();
        assert!(!replay.windows(4).any(|w| w == b"\x1b[6n"));
        let (seq, snapshot) = recovery.snapshot(&parser).unwrap();
        assert_eq!(seq, recovery.seq);
        // Repeated attachments, including two concurrent viewers, read only
        // canceled queries. The wire does not carry browser reply authority.
        for _ in 0..3 {
            let mut viewer = vt100::Parser::new(24, 80, 0);
            let mut replayed = Recovery::new();
            assert!(replayed.process(&mut viewer, &replay).is_empty());
            let mut restored = Recovery::new();
            assert!(restored.process(&mut viewer, &snapshot).is_empty());
        }
    }

    #[test]
    fn query_prefixes_can_span_every_read_boundary() {
        let bytes = b"before\x1b[2;7H\x1b[6nafter\x1b[?6n\x1b[5n\x1b[0c\x1b[>0c";
        for split in 0..=bytes.len() {
            let mut parser = vt100::Parser::new(24, 80, 0);
            let mut recovery = Recovery::new();
            let mut replies = recovery.process(&mut parser, &bytes[..split]);
            replies.extend(recovery.process(&mut parser, &bytes[split..]));
            assert_eq!(
                replies, b"\x1b[2;7R\x1b[?2;12R\x1b[0n\x1b[?1;2c\x1b[>0;0;0c",
                "split {split}"
            );
            assert!(parser.screen().contents().contains("after"));
        }
    }

    #[test]
    fn cursor_reports_respect_origin_mode_dimensions_and_pending_wrap() {
        let mut parser = vt100::Parser::new(6, 20, 0);
        let mut recovery = Recovery::new();
        assert_eq!(
            recovery.process(&mut parser, b"\x1b[2;5r\x1b[?6h\x1b[2;5H\x1b[6n"),
            b"\x1b[2;5R"
        );
        assert_eq!(
            recovery.process(
                &mut parser,
                b"\x1b[?6l\x1b[r\x1b[H12345678901234567890\x1b[6n"
            ),
            b"\x1b[1;20R"
        );
        parser.set_size(10, 30);
        recovery.resized();
        assert_eq!(
            recovery.process(&mut parser, b"\x1b[10;30H\x1b[6n"),
            b"\x1b[10;30R"
        );
    }

    #[test]
    fn embedded_and_malformed_queries_cannot_generate_replies() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        // CSI text inside OSC/DCS is data. ESC inside OSC can end it, so use
        // query-looking content without a new ESC as real opaque payloads do.
        for bytes in [
            b"\x1b]0;title [6n [c\x07".as_slice(),
            b"\x1bPq[6n[c\x1b\\".as_slice(),
            b"\x1b[6;7n\x1b[6:1n\x1b[>6n\x1b[1c".as_slice(),
            b"\x1b[6\x18n".as_slice(),
        ] {
            assert!(recovery.process(&mut parser, bytes).is_empty());
        }
        assert_eq!(recovery.process(&mut parser, b"\x1b[5n"), b"\x1b[0n");
    }

    #[test]
    fn snapshots_preserve_partial_ansi_utf8_and_alternate_screen() {
        recover(&[b"primary\x1b[?1049hTUI\x1b[3", b"1"]);
        recover(&[b"hello \xe4", b"\xb8", b"\xad\x1b[3"]);
        recover(&[b"before\x1b]0;title", b"\x1b", b"\\after\x1b[3"]);
        recover(&[b"before\x1b[3", b"\n", b"1"]);
    }
    #[test]
    fn overflowing_replay_keeps_parser_authoritative_and_reports_gap() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        for _ in 0..100 {
            recovery.process(&mut parser, &[b'x'; 8192]);
        }
        recovery.process(&mut parser, b"\r\nLAST_MARKER");
        assert_eq!(recovery.bytes.len(), REPLAY_BYTES);
        assert!(recovery.read(0).is_none());
        assert!(parser.screen().contents().contains("LAST_MARKER"));
        let (seq, snapshot) = recovery.snapshot(&parser).unwrap();
        assert_eq!(seq, recovery.seq);
        let mut restored = vt100::Parser::new(24, 80, 0);
        restored.process(&snapshot);
        assert_eq!(restored.screen().contents(), parser.screen().contents());
    }
    #[test]
    fn completed_history_releases_replay_and_probe_allocations() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        recovery.process(&mut parser, &[b'x'; 8192]);
        let final_seq = recovery.seq;
        recovery.clear();
        assert_eq!(recovery.bytes.capacity(), 0);
        assert!(recovery.probe.is_none());
        assert!(recovery.checkpoint.is_none());
        assert!(recovery.contains(final_seq));
        assert!(!recovery.contains(final_seq - 1));
    }
    #[test]
    fn long_incomplete_sequence_never_forges_recoverable_cursor() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        let mut recovery = Recovery::new();
        recovery.process(&mut parser, b"ok\x1b]");
        for _ in 0..70 {
            recovery.process(&mut parser, &[b'x'; 8192]);
        }
        assert!(recovery.snapshot(&parser).is_none());
        recovery.process(&mut parser, b"\x07done");
        assert!(recovery.snapshot(&parser).is_some());
    }
}
