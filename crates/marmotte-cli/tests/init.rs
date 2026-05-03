//! `marmotte init` smoke test.
//

use std::process::Command;

#[test]
fn init_creates_db_and_emits_token() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.toml");
    let cfg = format!(
        r#"
[server]
listen = "127.0.0.1:0"
storage_root = "{root}"
[database]
path = "{root}/marmotte.db"
[gc]
default_ttl_sstate_days = 30
default_ttl_downloads_days = 365
global_quota_bytes = 1073741824
"#,
        root = tmp.path().display()
    );
    std::fs::write(&cfg_path, cfg).unwrap();

    let out_token = tmp.path().join("admin.token");
    let exe = env!("CARGO_BIN_EXE_marmotte");
    let status = Command::new(exe)
        .args([
            "init",
            "--config",
            cfg_path.to_str().unwrap(),
            "--admin-token-out",
            out_token.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(tmp.path().join("marmotte.db").exists());
    let token = std::fs::read_to_string(&out_token).unwrap();
    assert!(token.trim().len() >= 40);
}
