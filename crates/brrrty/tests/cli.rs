use std::process::Command;

#[test]
fn version_flag_prints_name_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "brrrty 0.0.1"
    );
}

#[test]
fn unknown_subcommand_is_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .arg("frobnicate")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
