//! Command line tools: config, replay, navigation, BSP, ABI.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use lb_config::check::{check_file, yaml_files};

const USAGE: &str = "usage: lb-cli config check <file-or-dir>...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&refs) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[&str]) -> Result<bool> {
    match args {
        ["config", "check", paths @ ..] if !paths.is_empty() => Ok(config_check(paths)),
        _ => bail!(USAGE),
    }
}

fn config_check(paths: &[&str]) -> bool {
    let mut files = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            files.extend(yaml_files(path));
        } else {
            files.push(path.to_path_buf());
        }
    }
    let mut ok = true;
    for f in &files {
        match check_file(f) {
            Ok(kind) => println!("ok     {} ({kind})", f.display()),
            Err(e) => {
                println!("error  {e}");
                ok = false;
            }
        }
    }
    println!(
        "{} file(s) checked, {}",
        files.len(),
        if ok { "all valid" } else { "errors found" }
    );
    ok
}
