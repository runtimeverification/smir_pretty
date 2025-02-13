use std::env;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

fn main() -> Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 2 {
        bail!("Usage: cargo run --bin cargo_stable_mir_json -- <PATH-TO-STABLE-MIR-JSON-REPO>")
    }
    let repo_path: PathBuf = args[1].clone().into();
    if !repo_path.is_dir() {
        bail!(
            "Provided should be path to stable_mir_json repo, but {} is not a dir",
            repo_path.display()
        );
    }

    let hidden_dir = setup(repo_path)?;
    record_ld_library_path(&hidden_dir)?;
    add_run_script(&hidden_dir)
}

fn add_run_script(hidden_dir: &Path) -> Result<()> {
    let run_script_path = hidden_dir.join("run.sh");
    let mut run_script = std::fs::File::create(&run_script_path)?;
    writeln!(run_script, "#!/bin/bash")?;
    writeln!(run_script, "set -eu")?;
    writeln!(run_script)?;
    writeln!(
        run_script,
        "export LD_LIBRARY_PATH=$(cat ~/.stable_mir_json/ld_library_path)"
    )?;
    writeln!(
        run_script,
        "exec \"/home/daniel/.stable_mir_json/debug/stable_mir_json\" \"$@\""
    )?;

    // Set the script permissions to -rwxr-xr-x
    std::fs::set_permissions(run_script_path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn record_ld_library_path(hidden_dir: &Path) -> Result<()> {
    const LOADER_PATH: &str = "LD_LIBRARY_PATH";
    if let Some(paths) = env::var_os(LOADER_PATH) {
        // Note: kani filters the LD_LIBRARY_PATH, not sure why as it is working locally as is
        let mut ld_library_file = std::fs::File::create(hidden_dir.join("ld_library_path"))?;
        writeln!(ld_library_file, "{}", paths.to_str().unwrap())?;
    } else {
        bail!("Couldn't read LD_LIBRARY_PATH from env");
    }

    Ok(())
}

fn smir_json_dir() -> Result<PathBuf> {
    let home_dir = home::home_dir().expect("couldn't find home directory");
    if !home_dir.is_dir() {
        bail!(
            "got home directory `{}` which isn't a directory",
            home_dir.display()
        );
    }
    let smir_json_dir = home_dir.join(".stable_mir_json");
    Ok(smir_json_dir)
}

fn setup(repo_dir: PathBuf) -> Result<PathBuf> {
    let smir_json_dir = smir_json_dir()?;
    println!("Creating {} directory", smir_json_dir.display());
    std::fs::create_dir(&smir_json_dir)?;

    copy_artefacts(&repo_dir, &smir_json_dir)?;

    Ok(smir_json_dir)
}

fn copy_artefacts(repo_dir: &Path, smir_json_dir: &Path) -> Result<()> {
    let dev_dir = repo_dir.join("target/debug/");
    let dev_rlib = dev_dir.join("libstable_mir_json.rlib");

    let release_dir = repo_dir.join("target/release/");
    let release_rlib = release_dir.join("libstable_mir_json.rlib");

    if !dev_rlib.exists() && !release_rlib.exists() {
        bail!(
            "Neither dev rlib `{}`, nor release rlib `{}` exists",
            dev_dir.display(),
            release_dir.display()
        );
    }

    // Debug
    if dev_rlib.exists() {
        cp_artefacts_from_profile(dev_dir, smir_json_dir, Profile::Dev)?;
    }

    // Release
    if release_rlib.exists() {
        cp_artefacts_from_profile(release_dir, smir_json_dir, Profile::Release)?;
    }

    Ok(())
}

enum Profile {
    Dev,
    Release,
}

impl Profile {
    fn folder_as_string(&self) -> String {
        match self {
            Profile::Dev => "debug/".into(),
            Profile::Release => "release/".into(),
        }
    }
}

fn cp_artefacts_from_profile(
    profile_dir: PathBuf,
    smir_json_dir: &Path,
    profile: Profile,
) -> Result<()> {
    let rlib = profile_dir.join("libstable_mir_json.rlib");
    let bin = profile_dir.join("stable_mir_json");

    // Stable MIR JSON bin and rlib
    let smir_json_profile_dir = smir_json_dir.join(profile.folder_as_string());
    std::fs::create_dir(&smir_json_profile_dir)?;

    let smir_json_profile_rlib = smir_json_profile_dir.join("libstable_mir_json.rlib");
    println!(
        "Copying {} to {}",
        rlib.display(),
        smir_json_profile_rlib.display()
    );
    std::fs::copy(rlib, smir_json_profile_rlib)?;

    let smir_json_profile_bin = smir_json_profile_dir.join("stable_mir_json");
    println!(
        "Copying {} to {}",
        bin.display(),
        smir_json_profile_bin.display()
    );
    std::fs::copy(bin, smir_json_profile_bin)?;

    // Deps
    let smir_json_profile_deps_dir = smir_json_profile_dir.join("deps/");
    std::fs::create_dir(&smir_json_profile_deps_dir)?;

    let profile_deps_dir = profile_dir.join("deps/");
    if let Ok(entries) = std::fs::read_dir(profile_deps_dir) {
        for file in entries.flatten() {
            let file_path = file.path();

            if !file_path.is_file() {
                continue;
            }

            if let Some(ext) = file_path.extension() {
                if ext == "rlib" {
                    let smir_json_profile_deps_rlib =
                        smir_json_profile_deps_dir.join(file_path.file_name().unwrap());
                    println!(
                        "Copying {} to {}",
                        file_path.display(),
                        smir_json_profile_deps_rlib.display()
                    );
                    std::fs::copy(file_path, smir_json_profile_deps_rlib)?;
                }
            }
        }
    }

    Ok(())
}
