use std::process::Command;

fn main() {
    // Capture short git commit hash at compile time.
    // Available in code as env!("GIT_SHORT_HASH").
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    let hash = match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    };

    println!("cargo:rustc-env=GIT_SHORT_HASH={}", hash);
    // Re-run if HEAD changes (new commit)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}
