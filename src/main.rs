//! The `eider` command line.
//!
//! Quiet on success, one line per finding (`path:line:col: CODE message`),
//! and exit codes a script can rely on: `0` clean, `1` findings, `2` a usage
//! or project error. Keeping `1` and `2` apart lets CI tell a project that
//! breaks the contract from one that could not be checked at all.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use eider::check::check;
use eider::{config, project};

const USAGE: &str = "\
usage: eider check [DIR]    check the project in DIR (default: .)
       eider order [DIR]    print models in build order
       eider --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match command {
        "--version" | "-V" => {
            println!("eider {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "check" | "order" => match rest {
            [flag] if flag == "--help" || flag == "-h" => {
                println!("{USAGE}");
                ExitCode::SUCCESS
            }
            [] => run(command, Path::new(".")),
            // A leading `-` is an unknown flag, not a directory.
            [dir] if !dir.starts_with('-') => run(command, Path::new(dir)),
            _ => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        other => {
            eprintln!("eider: unknown command `{other}`\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(command: &str, root: &Path) -> ExitCode {
    let config_path = root.join("eider.toml");
    let text = match std::fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("eider: cannot read {}: {e}", config_path.display());
            return ExitCode::from(2);
        }
    };
    let config = match config::parse(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("eider: {e}");
            return ExitCode::from(2);
        }
    };
    let (models, misplaced) = match project::discover(root) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("eider: cannot read models: {e}");
            return ExitCode::from(2);
        }
    };
    let report = check(&config, &models, &misplaced);
    for d in &report.diagnostics {
        let (line, col) = d
            .span
            .and_then(|(start, _)| {
                models
                    .iter()
                    .find(|m| m.path == d.path)
                    .map(|m| line_col(&m.sql, start))
            })
            .unwrap_or((1, 1));
        println!(
            "{}:{line}:{col}: {} {}",
            display(root, &d.path).display(),
            d.code,
            d.message
        );
    }
    if !report.diagnostics.is_empty() {
        return ExitCode::from(1);
    }
    // `order` prints nothing for a project with findings: its order is only
    // meaningful once the contract holds.
    if command == "order" {
        for name in &report.order {
            println!("{name}");
        }
    }
    ExitCode::SUCCESS
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

fn display(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}
