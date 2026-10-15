use std::process::Command;

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ergent-server"))
        .args(args)
        .output()
        .expect("server help runs");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn server_help_is_concise_bilingual_and_explains_package_transport() {
    let top = help(&["--help"]);
    assert!(top.contains("服务端 / Server"));
    assert!(top.contains("start-server.sh"));
    assert!(top.contains("--web ./web"));
    assert!(top.contains("Same database"));
    for expected in [
        "详细参数 / Details:",
        "ergent-server run --help",
        "ergent-server setup-code --help",
        "ergent-server admin-create --help",
        "Nginx HTTPS",
        "127.0.0.1:7777",
        "./ergent-server run --web ./web --tls-mode proxy --origin https://example.com",
        "./start-server.sh --tls-mode proxy --origin https://example.com",
        "Replace example.com",
        "does not enable TLS",
    ] {
        assert!(top.contains(expected), "missing top-level help: {expected}");
    }
    assert_eq!(top, help(&["-h"]));
    assert!(top.lines().count() < 45);
    let run = help(&["run", "--help"]);
    for expected in [
        "启动服务 / Start server",
        "does not enable TLS",
        "direct",
        "proxy",
        "dev-http",
        "loopback HTTP",
        "admin-create",
        "./web",
    ] {
        assert!(run.contains(expected), "missing help: {expected}");
    }
    assert!(!run.contains("A fixed failure window"));
    assert!(run.lines().count() < 100);
    let admin = help(&["admin-create", "--help"]);
    assert!(admin.contains("隐藏密码"));
    assert!(admin.contains("Enter, then confirm"));
}

#[cfg(unix)]
#[test]
fn package_launcher_serves_web_from_another_working_directory() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package with spaces");
    std::fs::create_dir_all(package.join("web")).unwrap();
    std::fs::write(
        package.join("web/index.html"),
        "<html>package-web-ok</html>",
    )
    .unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_ergent-server"),
        package.join("ergent-server"),
    )
    .unwrap();
    std::fs::write(
        package.join("start-server.sh"),
        include_str!("../../../scripts/release/start-server.sh"),
    )
    .unwrap();
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let database = temp.path().join("explicit-data/server.db");
    let mut child = Child(
        Command::new("sh")
            .arg(package.join("start-server.sh"))
            .args([
                "--database",
                database.to_str().unwrap(),
                "--tls-mode",
                "dev-http",
                "--listen",
                &address.to_string(),
                "--origin",
                &format!("http://{address}"),
            ])
            .current_dir(temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stream = loop {
        if let Ok(stream) = TcpStream::connect(address) {
            break stream;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "launcher exited early"
        );
        assert!(Instant::now() < deadline, "launcher never became ready");
        thread::sleep(Duration::from_millis(20));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("package-web-ok"));
    assert!(database.is_file());
    assert!(!package.join(".local/server.db").exists());
}
