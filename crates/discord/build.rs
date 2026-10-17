use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=DISCORD_CLIENT_ID");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let Some(root) = manifest.parent().and_then(|p| p.parent()) else {
        return;
    };
    let env_path = root.join(".env");

    // Only watch `.env` when it is actually there. Two separate Cargo behaviours
    // make the obvious version of this actively harmful:
    //
    //   * a `rerun-if-changed` path that does not exist counts as changed on
    //     every check, and
    //   * a `rerun-if-changed` path that *is* a directory is scanned
    //     recursively — and the repo root contains `target/`, which Cargo
    //     rewrites on every build.
    //
    // Either one marks this crate dirty on every build, and because the
    // application depends on it, that recompiles and relinks the whole app for
    // nothing. (Measured: a no-op build cost 4m08s before this.)
    //
    // The cost of this version: creating `.env` for the first time is not
    // noticed until something else dirties this crate. `touch this file` or
    // `cargo clean -p discord` after creating it.
    if env_path.exists() {
        println!("cargo:rerun-if-changed={}", env_path.display());
    }

    let Ok(contents) = std::fs::read_to_string(&env_path) else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key != "DISCORD_CLIENT_ID" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
