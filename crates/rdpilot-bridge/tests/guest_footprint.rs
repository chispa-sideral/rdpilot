//! The bridge changes nothing outside its per-user directory: no registry,
//! service, scheduled task, PATH or firewall API appears in its source.
use std::path::Path;

#[test]
fn bridge_source_uses_no_machine_or_profile_wide_api() {
    // Assembled at run time so this file does not match itself.
    let forbidden: Vec<String> = [
        ["Reg", "CreateKey"],
        ["Reg", "SetValue"],
        ["Reg", "OpenKey"],
        ["Win32_System_", "Registry"],
        ["Create", "Service"],
        ["Open", "SCManager"],
        ["ITask", "Service"],
        ["Set", "EnvironmentVariable"],
        ["INet", "FwPolicy"],
        ["netsh", " "],
        ["schtasks", ""],
        ["set", "_var(\"PATH"],
    ]
    .iter()
    .map(|parts| parts.concat())
    .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("Cargo.toml")];
    for entry in std::fs::read_dir(root.join("src")).unwrap() {
        files.push(entry.unwrap().path());
    }
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for needle in &forbidden {
            assert!(
                !text.contains(needle.as_str()),
                "{} uses {needle}",
                file.display()
            );
        }
    }
}
