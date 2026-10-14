//! Snapshot continuation fixtures consumed by the real xterm parser tests.
#[path = "../src/screen.rs"]
mod screen;
use base64::{engine::general_purpose::STANDARD, Engine};
fn main() {
    let cases: &[(&str, &[u8], &[u8])] = &[
        (
            "margins",
            b"TOP\x1b[2;1HMIDDLE\x1b[5;1HBOTTOM\x1b[2;4r\x1b[4;1Hend",
            b"\r\nnext",
        ),
        (
            "saved cursor and SGR",
            b"\x1b[2;5H\x1b[31m\x1b7\x1b[32m\x1b[4;8Hnow",
            b"\x1b8X",
        ),
        ("origin", b"TOP\x1b[2;4r\x1b[?6hbody", b"\x1b[Hnew"),
        (
            "saved origin",
            b"\x1b[2;5r\x1b[?6h\x1b[2;5H\x1b7\x1b[?6l\x1b[3;6r\x1b[1;1Hnow",
            b"\x1b8X\r\nnext",
        ),
        (
            "alternate exit",
            b"primary\x1b[2;4r\x1b[?6h\x1b[?1049h\x1b[?6l\x1b[1;1Halt\x1b[3;5r\x1b[3;1H",
            b"\x1b[5;1Hend\r\nnext\x1b[?1049lX",
        ),
        ("pending wrap", b"12345678901234567890", b"X"),
    ];
    println!("[");
    for (index, (name, setup, suffix)) in cases.iter().enumerate() {
        let mut parser = vt100::Parser::new(6, 20, 0);
        parser.process(setup);
        println!(
            "{}{{\"name\":\"{}\",\"setup\":\"{}\",\"snapshot\":\"{}\",\"suffix\":\"{}\"}}",
            if index == 0 { "" } else { "," },
            name,
            STANDARD.encode(setup),
            STANDARD.encode(screen::snapshot(parser.screen())),
            STANDARD.encode(suffix)
        );
    }
    println!("]");
}
