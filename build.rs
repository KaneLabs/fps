use std::process::Command;

fn main() {
    // --- Git short hash (backwards-compatible: GIT_SHORT_HASH) ---
    let hash = cmd("git", &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHORT_HASH={hash}");

    // --- ANIMA_VERSION: release tag (e.g. "v0.3.0-a1b2c3d") ---
    // CI sets ANIMA_VERSION env var; local builds derive from Cargo.toml + git hash.
    let version = std::env::var("ANIMA_VERSION").unwrap_or_else(|_| {
        format!("v{}-{}", env!("CARGO_PKG_VERSION"), hash)
    });
    println!("cargo:rustc-env=ANIMA_VERSION={version}");

    // --- ANIMA_BUILD_SHA: short commit hash ---
    let sha = std::env::var("ANIMA_BUILD_SHA").unwrap_or_else(|_| hash.clone());
    println!("cargo:rustc-env=ANIMA_BUILD_SHA={sha}");

    // --- ANIMA_BUILD_DATE: ISO 8601 UTC timestamp ---
    let date = std::env::var("ANIMA_BUILD_DATE").unwrap_or_else(|_| {
        cmd("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])
            .unwrap_or_else(|| "unknown".to_string())
    });
    println!("cargo:rustc-env=ANIMA_BUILD_DATE={date}");

    // Re-run triggers
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-env-changed=ANIMA_VERSION");
    println!("cargo:rerun-if-env-changed=ANIMA_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=ANIMA_BUILD_DATE");
}

fn cmd(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}
