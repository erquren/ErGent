//! Complete ConPTY cursor inheritance in the Agent, before any viewer attaches.
use std::io::{self, Write};

const QUERY: &[u8] = b"\x1b[6n";
const ORIGIN: &[u8] = b"\x1b[1;1R";

#[derive(Default)]
pub(crate) struct CursorHandshake {
    pending: Vec<u8>,
    complete: bool,
}

impl CursorHandshake {
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn process(&mut self, bytes: &[u8], writer: &mut dyn Write) -> io::Result<Vec<u8>> {
        if self.complete {
            return Ok(bytes.to_vec());
        }
        self.pending.extend_from_slice(bytes);
        if let Some(index) = self.pending.windows(QUERY.len()).position(|w| w == QUERY) {
            // Each session starts on a fresh screen. Answer locally: a browser
            // may not exist yet, and its recovery snapshot cannot retain DSR.
            writer.write_all(ORIGIN)?;
            writer.flush()?;
            self.complete = true;
            // Do not replay this startup query to viewers; a second answer
            // could otherwise be interpreted as shell input.
            self.pending.drain(index..index + QUERY.len());
            return Ok(std::mem::take(&mut self.pending));
        }
        // Retain only a possible query prefix, including across read boundaries.
        let retained = (1..QUERY.len())
            .rev()
            .find(|&len| self.pending.ends_with(&QUERY[..len]))
            .unwrap_or(0);
        let end = self.pending.len() - retained;
        Ok(self.pending.drain(..end).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_query_is_answered_and_removed_at_every_chunk_boundary() {
        let bytes = b"before\x1b[6nafter";
        for split in 0..=bytes.len() {
            let mut handshake = CursorHandshake::default();
            let mut replies = Vec::new();
            let mut output = handshake.process(&bytes[..split], &mut replies).unwrap();
            output.extend(handshake.process(&bytes[split..], &mut replies).unwrap());
            assert_eq!(output, b"beforeafter");
            assert_eq!(replies, ORIGIN);
            assert!(handshake.is_complete());
            // Later application queries belong to the normal terminal stream.
            assert_eq!(handshake.process(QUERY, &mut replies).unwrap(), QUERY);
            assert_eq!(replies, ORIGIN);
        }
    }

    #[test]
    fn bytewise_reads_preserve_other_escapes_and_utf8() {
        let bytes = "\x1b[31m中文\x1b\x1b[6n\x1b[0m".as_bytes();
        let mut handshake = CursorHandshake::default();
        let mut replies = Vec::new();
        let mut output = Vec::new();
        for byte in bytes {
            output.extend(handshake.process(&[*byte], &mut replies).unwrap());
        }
        assert_eq!(output, "\x1b[31m中文\x1b\x1b[0m".as_bytes());
        assert_eq!(replies, ORIGIN);
    }

    #[test]
    fn reply_errors_are_propagated() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut handshake = CursorHandshake::default();
        assert!(handshake.process(QUERY, &mut BrokenWriter).is_err());
        assert!(!handshake.is_complete());
    }
}
