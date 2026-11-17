//! `brrty` only points the way to `brrrty`.

#[test]
fn it_names_the_real_crate_and_exits_0() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_brrty"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "You probably want brrrty (three Rs, like \"go brrr\"):\n  cargo install brrrty\n"
    );
    assert!(out.stderr.is_empty());
}
