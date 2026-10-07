//! Isolates the preview screen engine so a more complete emulator can replace it.
//! vt100's state_formatted alone does NOT restore the alternate buffer.
pub(crate) fn snapshot(screen: &vt100::Screen) -> Vec<u8> {
    if !screen.alternate_screen() {
        return screen.state_formatted();
    }
    let mut primary = screen.clone();
    let mut parser = vte::Parser::new();
    for byte in b"\x1b[?1049l" {
        parser.advance(&mut primary, *byte);
    }
    let mut data = primary.state_formatted();
    data.extend_from_slice(b"\x1b[?1049h");
    data.extend(screen.state_formatted());
    data
}
