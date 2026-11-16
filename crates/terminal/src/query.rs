//! Replies owned by the Agent, not by whichever browser happens to be watching.
//!
//! Only recognize complete CSI queries parsed by VTE. The live output path
//! cancels recognized queries before replay so no viewer can answer them twice.
#[derive(Clone, Copy)]
pub(crate) enum Query {
    Status,
    Cursor { private: bool },
    PrimaryAttributes,
    SecondaryAttributes,
}
impl Query {
    pub fn parse(
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) -> Option<Self> {
        if ignore {
            return None;
        }
        let mut values = params.iter();
        let value = values.next()?;
        if value.len() != 1 || values.next().is_some() {
            return None;
        }
        match (intermediates, value[0], action) {
            (b"", 5, 'n') => Some(Self::Status),
            (b"", 6, 'n') => Some(Self::Cursor { private: false }),
            (b"?", 6, 'n') => Some(Self::Cursor { private: true }),
            (b"", 0, 'c') => Some(Self::PrimaryAttributes),
            (b">", 0, 'c') => Some(Self::SecondaryAttributes),
            _ => None,
        }
    }

    pub fn reply(self, screen: &vt100::Screen) -> Vec<u8> {
        match self {
            Self::Status => b"\x1b[0n".to_vec(),
            Self::Cursor { private } => {
                let (mut row, col) = screen.cursor_position();
                // CPR coordinates are relative to the scroll region in DECOM.
                let mut probe = screen.clone();
                let mut parser = vte::Parser::new();
                for byte in b"\x1b[H" {
                    parser.advance(&mut probe, *byte);
                }
                row = row.saturating_sub(probe.cursor_position().0);
                // A pending autowrap is represented by x == cols by vt100, but
                // VT cursor reports refer to the last visible column.
                let col = col.min(screen.size().1.saturating_sub(1));
                format!(
                    "\x1b[{}{};{}R",
                    if private { "?" } else { "" },
                    row + 1,
                    col + 1
                )
                .into_bytes()
            }
            // Advertise only basic VT100 capabilities. The authoritative model
            // cannot preserve xterm-only extensions through a fresh attachment.
            Self::PrimaryAttributes => b"\x1b[?1;2c".to_vec(),
            Self::SecondaryAttributes => b"\x1b[>0;0;0c".to_vec(),
        }
    }
}
