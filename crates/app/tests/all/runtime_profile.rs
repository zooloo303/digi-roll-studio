use std::process::Command;

#[test]
fn executable_selects_storage_and_autoconnect_before_starting_services() {
    let output = Command::new(env!("CARGO_BIN_EXE_digi_roll_studio"))
        .arg("--runtime-info")
        .output()
        .expect("run startup diagnostics");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let preview = cfg!(feature = "plugin-host");
    let directory = if preview {
        "digi-roll-studio-plugin-preview"
    } else {
        "digi-roll-studio"
    };
    let root = std::path::Path::new(info["data_root"].as_str().unwrap());
    assert!(root.is_absolute());
    assert!(root.ends_with(directory));
    assert_eq!(info["hardware_autoconnect"], !preview);
    assert_eq!(info["plugin_host_feature"], preview);
    assert_eq!(
        std::path::Path::new(info["backups"].as_str().unwrap()),
        root.join("backups")
    );
    assert_eq!(
        std::path::Path::new(info["preset_index"].as_str().unwrap()),
        root.join("preset-index")
    );
    assert_eq!(
        std::path::Path::new(info["recovery"].as_str().unwrap()),
        root.join("recovery")
    );
}
