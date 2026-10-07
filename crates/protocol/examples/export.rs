fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/protocol-ts/index.ts");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, ergent_protocol::typescript()).unwrap();
}
