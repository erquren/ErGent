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
    pub fn process(&mut self, parser: &mut vt100::Parser, bytes: &[u8]) {
        let was_ground = self.ground;
        let mut boundary = None;
        for (index, &byte) in bytes.iter().enumerate() {
            let mut probe = Boundary(self.ground && (byte < 0x20 || byte == 0x7f) && byte != 0x1b);
            self.probe
                .as_mut()
                .expect("live parser probe")
                .advance(&mut probe, byte);
            self.ground = probe.0;
            if self.ground {
                boundary = Some(index + 1);
            }
        }
        if let Some(end) = boundary {
            parser.process(&bytes[..end]);
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
struct Boundary(bool);
impl vte::Perform for Boundary {
    fn print(&mut self, _: char) {
        self.0 = true;
    }
    fn execute(&mut self, byte: u8) {
        if byte == 0x18 || byte == 0x1a {
            self.0 = true;
        }
    }
    fn csi_dispatch(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {
        self.0 = true;
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
        self.0 = true;
    }
    fn osc_dispatch(&mut self, _: &[&[u8]], bell: bool) {
        self.0 = bell;
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
