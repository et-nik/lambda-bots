use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    match args.first().map(String::as_str) {
        Some("layering") => layering(&root),
        Some("abi") => abi(&root, args.iter().any(|a| a == "--check")),
        Some("check-binary") => check_binary(&root, &args[1..]),
        Some("package") => package(&root, &args[1..]),
        _ => {
            eprintln!(
                "usage: cargo xtask <layering | abi [--check] | check-binary <file>... | \
                 package --version V --out DIR [--linux SO] [--windows DLL] [--macos DYLIB]>"
            );
            std::process::exit(2);
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf()
}

/// Crates whose transitive dependencies may include `lb-raw`.
const RAW_ALLOWED: &[&str] = &[
    "lb-raw",
    "lb-host",
    "lb-perception",
    "lb-brain",
    "lb-runtime",
    "lb-plugin",
    "lb-testkit",
    "lb-cli",
    "xtask",
];

/// Crates that must never see the engine layer (`lb-host`, `lb-ffi`).
const ENGINE_FREE: &[&str] = &[
    "lb-core",
    "lb-raw",
    "lb-worldq",
    "lb-config",
    "lb-telemetry",
    "lb-game",
    "lb-bsp",
    "lb-mapmesh",
    "lb-editor",
    "lb-kin",
    "lb-nav-api",
    "lb-nav",
    "lb-navgen",
    "lb-mapknow",
    "lb-perception",
    "lb-knowledge",
    "lb-styles",
    "lb-motor",
    "lb-combat",
    "lb-actions",
    "lb-decision",
    "lb-modes",
    "lb-brain",
    "lb-ext",
    "lb-chat",
    "lb-llm",
];

fn layering(root: &Path) -> Result<()> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
        .context("cargo metadata")?;
    let resolve = metadata.resolve.as_ref().context("no resolve graph")?;
    let names: BTreeMap<_, _> = metadata
        .packages
        .iter()
        .map(|p| (p.id.clone(), p.name.to_string()))
        .collect();
    let workspace: BTreeSet<_> = metadata.workspace_members.iter().cloned().collect();
    let edges: BTreeMap<_, Vec<_>> = resolve
        .nodes
        .iter()
        .map(|n| {
            (
                n.id.clone(),
                n.deps.iter().filter(|d| is_normal(d)).map(|d| d.pkg.clone()).collect(),
            )
        })
        .collect();

    let mut violations = Vec::new();
    for id in &workspace {
        let name = &names[id];
        let closure = closure(id, &edges);
        let internal: BTreeSet<&str> = closure
            .iter()
            .filter(|d| workspace.contains(*d))
            .map(|d| names[*d].as_str())
            .collect();
        if internal.contains("lb-raw") && !RAW_ALLOWED.contains(&name.as_str()) {
            violations.push(format!(
                "{name}: must not depend on lb-raw (raw sensor data is perception-only)"
            ));
        }
        if ENGINE_FREE.contains(&name.as_str()) {
            for forbidden in ["lb-host", "lb-ffi"] {
                if internal.contains(forbidden) {
                    violations.push(format!("{name}: must not depend on {forbidden}"));
                }
            }
        }
        if name == "lb-ffi" && !internal.is_empty() {
            violations.push(format!(
                "lb-ffi: must not depend on workspace crates, found {internal:?}"
            ));
        }
    }
    if violations.is_empty() {
        println!("layering: ok ({} workspace crates)", workspace.len());
        Ok(())
    } else {
        for v in &violations {
            eprintln!("layering violation: {v}");
        }
        bail!("{} layering violation(s)", violations.len())
    }
}

fn is_normal(dep: &cargo_metadata::NodeDep) -> bool {
    dep.dep_kinds
        .iter()
        .any(|k| k.kind == cargo_metadata::DependencyKind::Normal)
}

fn closure<'a, K: Ord>(start: &'a K, edges: &'a BTreeMap<K, Vec<K>>) -> BTreeSet<&'a K> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&K> = edges.get(start).map(|v| v.iter().collect()).unwrap_or_default();
    while let Some(k) = stack.pop() {
        if seen.insert(k)
            && let Some(next) = edges.get(k)
        {
            stack.extend(next.iter());
        }
    }
    seen
}

fn abi(root: &Path, check: bool) -> Result<()> {
    let jobs = [
        ("crates/lb-ffi", "adapter/include/lb/lb_abi.h"),
        ("crates/lb-plugin", "adapter/include/lb/lb_core.h"),
    ];
    let mut stale = Vec::new();
    for (krate, header) in jobs {
        let config = cbindgen::Config::from_file(root.join(krate).join("cbindgen.toml"))
            .map_err(|e| anyhow::anyhow!("{krate}/cbindgen.toml: {e}"))?;
        let bindings = cbindgen::Builder::new()
            .with_crate(root.join(krate))
            .with_config(config)
            .generate()
            .with_context(|| format!("cbindgen generation failed for {krate}"))?;
        let mut generated = Vec::new();
        bindings.write(&mut generated);
        let path = root.join(header);
        if check {
            let current = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            if current != generated {
                stale.push(header);
            }
        } else {
            std::fs::write(&path, &generated).with_context(|| format!("write {}", path.display()))?;
            println!("abi: wrote {header}");
        }
    }
    if !stale.is_empty() {
        bail!("out of date: {} (run `cargo xtask abi`)", stale.join(", "));
    }
    if check {
        println!("abi: headers are up to date");
    }
    Ok(())
}

const EXPECTED_EXPORTS: &[&str] = &[
    "GiveFnptrsToDll",
    "Meta_Init",
    "Meta_Query",
    "Meta_Attach",
    "Meta_Detach",
];

fn check_binary(root: &Path, files: &[String]) -> Result<()> {
    if files.is_empty() {
        bail!("usage: cargo xtask check-binary <file>...");
    }
    let script = root.join("scripts/check-binary.sh");
    for file in files {
        let status = Command::new("bash")
            .arg(&script)
            .arg(file)
            .status()
            .context("run check-binary.sh")?;
        if !status.success() {
            bail!(
                "{file}: binary check failed (expected exports: {})",
                EXPECTED_EXPORTS.join(", ")
            );
        }
    }
    Ok(())
}

/// Files in `addons/lambdabots/` of every package, at the same paths as in the repository.
const PACKAGE_DOCS: &[&str] = &[
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_LICENSES.md",
    "README.md",
    "docs/personas.md",
    "docs/perception.md",
    "docs/behavior.md",
    "docs/navigation.md",
    "docs/overlays.md",
    "docs/replay.md",
    "docs/chat.md",
    "docs/images/replay.svg",
];
/// Data directories copied into `addons/lambdabots/`.
const PACKAGE_DATA: &[&str] = &["config", "names", "profiles", "maps", "amxx"];

/// `dist/lambdabots-<version>-<platform>.{tar.gz,zip}` with the `addons/lambdabots/` layout.
fn package(root: &Path, args: &[String]) -> Result<()> {
    let mut version = None;
    let mut out = root.join("dist");
    let mut modules = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--version" => version = Some(value()?),
            "--out" => out = PathBuf::from(value()?),
            "--linux" => modules.push(("linux-i386", PathBuf::from(value()?), "linux")),
            "--windows" => modules.push(("windows-x86", PathBuf::from(value()?), "win32")),
            "--macos" => modules.push(("macos-arm64", PathBuf::from(value()?), "osx")),
            other => bail!("package: unknown option {other}"),
        }
    }
    let version = version.context("package: --version is required")?;
    if modules.is_empty() {
        bail!("package: pass at least one of --linux, --windows, --macos");
    }
    std::fs::create_dir_all(&out)?;
    for (platform, module, ini_os) in modules {
        let name = format!("lambdabots-{version}-{platform}");
        let stage = out.join(&name);
        if stage.exists() {
            std::fs::remove_dir_all(&stage)?;
        }
        let addon = stage.join("addons/lambdabots");
        std::fs::create_dir_all(addon.join("bin"))?;
        std::fs::create_dir_all(addon.join("logs"))?;
        let file_name = module.file_name().context("module path has no file name")?;
        std::fs::copy(&module, addon.join("bin").join(file_name))
            .with_context(|| format!("copy {}", module.display()))?;
        for dir in PACKAGE_DATA {
            copy_tree(&root.join("data").join(dir), &addon.join(dir))?;
        }
        for doc in PACKAGE_DOCS {
            let src = root.join(doc);
            if src.exists() {
                let dst = addon.join(doc);
                if let Some(dir) = dst.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::copy(&src, dst)?;
            }
        }
        std::fs::write(
            addon.join("plugins.ini.txt"),
            format!("{ini_os} addons/lambdabots/bin/{}\n", file_name.to_string_lossy()),
        )?;
        let archive = if platform.starts_with("windows") {
            let zip = out.join(format!("{name}.zip"));
            let _ = std::fs::remove_file(&zip);
            run(Command::new("zip")
                .arg("-qr")
                .arg(&zip)
                .arg("addons")
                .current_dir(&stage))?;
            zip
        } else {
            let tgz = out.join(format!("{name}.tar.gz"));
            run(Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg(&tgz)
                .arg("-C")
                .arg(&stage)
                .arg("addons"))?;
            tgz
        };
        println!("package: {}", archive.display());
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("run {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}
