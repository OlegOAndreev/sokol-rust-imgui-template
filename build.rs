use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SHADER_DIR: &str = "shaders";
const SRC_DIR: &str = "src/shaders";
const TARGETS: &str = "glsl430:glsl300es:metal_macos:hlsl5";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shader_dir = manifest_dir.join(SHADER_DIR);
    let src_dir = manifest_dir.join(SRC_DIR);
    let tools_base = manifest_dir.join("tools/sokol-tools-bin/bin");

    let shdc_bin = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "aarch64") => tools_base.join("linux_arm64/sokol-shdc"),
        ("linux", _) => tools_base.join("linux/sokol-shdc"),
        ("macos", "aarch64") => tools_base.join("osx_arm64/sokol-shdc"),
        ("macos", _) => tools_base.join("osx/sokol-shdc"),
        ("windows", _) => tools_base.join("win32/sokol-shdc.exe"),
        _ => panic!(
            "unsupported platform: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    };
    if !shdc_bin.exists() {
        panic!("sokol-shdc not found at {}", shdc_bin.display());
    }

    // Auto-discover all .glsl files in the shaders directory
    let mut shader_names = vec![];
    let entries = fs::read_dir(&shader_dir).expect("failed to read shaders/ directory");
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "glsl") {
            let name = path
                .file_stem()
                .expect("file has no stem")
                .to_str()
                .expect("non-UTF-8 filename")
                .to_string();
            shader_names.push(name);
        }
    }
    shader_names.sort();
    println!("cargo:rerun-if-changed={}", shader_dir.display());

    // Compile each shader with sokol-shdc
    for name in &shader_names {
        let input = shader_dir.join(format!("{}.glsl", name));
        let output = src_dir.join(format!("{}_shader.rs", name));

        let status = Command::new(&shdc_bin)
            .arg("-i")
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .args(["-l", TARGETS])
            .args(["-f", "sokol_rust"])
            .status()
            .expect("failed to run sokol-shdc");

        if !status.success() {
            panic!("sokol-shdc failed for {}/{}.glsl", SHADER_DIR, name);
        }
    }

    // Write mod.rs if the contents has changed
    let mod_rs_path = src_dir.join("mod.rs");
    let mod_rs_content: String = shader_names
        .iter()
        .map(|name| format!("pub mod {}_shader;\n", name))
        .collect();

    let existing_mod_rs = fs::read_to_string(&mod_rs_path).unwrap_or_default();
    if existing_mod_rs != mod_rs_content {
        fs::write(&mod_rs_path, &mod_rs_content).expect("failed to write mod.rs");
        println!("cargo:warning=updated shaders/mod.rs");
    }
}
