//! Isolates the preview screen engine so a more complete emulator can replace it.
//! vt100's state_formatted omits the inactive buffer, margins and saved cursor.
fn apply(screen: &mut vt100::Screen, data: &[u8]) {
    let mut parser = vte::Parser::new();
    for &byte in data {
        parser.advance(screen, byte);
    }
}
fn origin(screen: &vt100::Screen) -> bool {
    let mut probe = screen.clone();
    // A nonzero temporary margin makes DECOM observable without private fields.
    apply(&mut probe, b"\x1b[2;3r\x1b[H");
    probe.cursor_position().0 != 0
}
fn margins(screen: &vt100::Screen) -> (u16, u16) {
    let mut probe = screen.clone();
    apply(&mut probe, b"\x1b[?6h\x1b[H");
    let top = probe.cursor_position().0;
    apply(&mut probe, b"\x1b[65535;1H");
    (top, probe.cursor_position().0)
}
fn mode(data: &mut Vec<u8>, origin: bool) {
    data.extend_from_slice(if origin { b"\x1b[?6h" } else { b"\x1b[?6l" });
}
fn cursor(screen: &vt100::Screen, top: u16) -> Vec<u8> {
    // vt100 emits absolute CUP positions even while DECOM is enabled. Translate
    // only its generated CUP rows; preserve SGR, Unicode and pending-wrap redraws.
    let bytes = screen.cursor_state_formatted();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"\x1b[") {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start && bytes.get(end) == Some(&b';') {
                let mut col = end + 1;
                while col < bytes.len() && bytes[col].is_ascii_digit() {
                    col += 1;
                }
                if bytes.get(col) == Some(&b'H') {
                    let row = std::str::from_utf8(&bytes[start..end])
                        .unwrap()
                        .parse::<u16>()
                        .unwrap();
                    out.extend_from_slice(
                        format!("\x1b[{}", row.saturating_sub(top).max(1)).as_bytes(),
                    );
                    out.extend_from_slice(&bytes[end..=col]);
                    i = col + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}
fn buffer(screen: &vt100::Screen) -> Vec<u8> {
    let current_origin = origin(screen);
    let (top, bottom) = margins(screen);
    let mut saved = screen.clone();
    apply(&mut saved, b"\x1b8");
    let saved_origin = origin(&saved);
    let mut data = b"\x1b[?6l\x1b[r".to_vec();
    data.extend(screen.state_formatted());
    // Save with full margins so a saved cursor can legitimately lie outside the
    // *current* margins. DECSC also saves SGR attributes and origin mode.
    mode(&mut data, saved_origin);
    data.extend(saved.cursor_state_formatted());
    data.extend(saved.attributes_formatted());
    data.extend_from_slice(b"\x1b7");
    data.extend_from_slice(format!("\x1b[{};{}r", top + 1, bottom + 1).as_bytes());
    mode(&mut data, current_origin);
    data.extend(cursor(screen, if current_origin { top } else { 0 }));
    data.extend(screen.attributes_formatted());
    data
}
pub(crate) fn snapshot(screen: &vt100::Screen) -> Vec<u8> {
    if !screen.alternate_screen() {
        return buffer(screen);
    }
    let mut primary = screen.clone();
    apply(&mut primary, b"\x1b[?47l");
    let mut data = b"\x1b[?47l".to_vec();
    data.extend(buffer(&primary));
    data.extend_from_slice(b"\x1b[?47h");
    data.extend(buffer(screen));
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    fn continuation(setup: &[u8], suffix: &[u8]) {
        let mut source = vt100::Parser::new(6, 20, 0);
        source.process(setup);
        let mut restored = vt100::Parser::new(6, 20, 0);
        restored.process(&snapshot(source.screen()));
        assert_eq!(restored.screen().contents(), source.screen().contents());
        source.process(suffix);
        restored.process(suffix);
        assert_eq!(restored.screen().contents(), source.screen().contents());
        assert_eq!(
            restored.screen().cursor_position(),
            source.screen().cursor_position()
        );
        assert_eq!(
            restored.screen().attributes_formatted(),
            source.screen().attributes_formatted()
        );
    }
    #[test]
    fn snapshots_restore_continuation_state_not_only_visible_cells() {
        continuation(
            b"TOP\x1b[2;1HMIDDLE\x1b[5;1HBOTTOM\x1b[2;4r\x1b[4;1Hend",
            b"\r\nnext",
        );
        continuation(b"\x1b[2;5H\x1b[31m\x1b7\x1b[32m\x1b[4;8Hnow", b"\x1b8X");
        continuation(b"TOP\x1b[2;4r\x1b[?6hbody", b"\x1b[Hnew");
        continuation(
            b"\x1b[2;5r\x1b[?6h\x1b[2;5H\x1b7\x1b[?6l\x1b[3;6rnow",
            b"\x1b8X\r\nnext",
        );
        continuation(
            b"primary\x1b[2;4r\x1b[?6h\x1b[?1049halt\x1b[3;5r",
            b"\x1b[5;1Hend\r\nnext\x1b[?1049lX",
        );
        continuation(b"12345678901234567890", b"X");
    }
}
