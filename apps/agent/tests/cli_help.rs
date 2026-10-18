use std::process::Command;

#[test]
fn agent_help_is_concise_bilingual_and_explains_hidden_input_and_ca() {
    for args in [vec!["--help"], vec!["enroll", "--help"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_ergent-client"))
            .args(&args)
            .output()
            .expect("agent help runs");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("ergent-client"));
        assert!(!text.contains("ergent-agent"));
        for expected in [
            "隐藏令牌",
            "hidden token",
            "--ca-cert",
            "HTTPS/WSS",
            "--config",
        ] {
            assert!(text.contains(expected), "missing help: {expected}");
        }
        assert!(text.lines().count() < 60);
    }
}
