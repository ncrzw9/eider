//! The `eider` command line.
//!
//! Quiet on success, one line per finding (`path:line:col: CODE message`),
//! and exit codes a script can rely on: `0` clean, `1` findings, `2` a usage
//! or project error. Keeping `1` and `2` apart lets CI tell a project that
//! breaks the contract from one that could not be checked at all.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use eider::check::check;
use eider::{config, project, run};

/// `println!` for stdout, ending the run when the reader has gone. Rust
/// ignores SIGPIPE, so a plain `println!` panics when output is piped into
/// something that stops reading (`eider order | head`). This exits with 141,
/// the status a Unix tool killed by SIGPIPE reports.
macro_rules! outln {
    ($($arg:tt)*) => { emit(format_args!($($arg)*)) };
}

fn emit(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = stdout
        .write_fmt(args)
        .and_then(|()| stdout.write_all(b"\n"))
    {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        eprintln!("eider: cannot write output: {e}");
        std::process::exit(2);
    }
}

const USAGE: &str = "\
usage: eider check [DIR]      check the project in DIR (default: .)
       eider order [DIR]      print models in build order
       eider compile [DIR]    print the SQL a run executes
       eider run [DIR] [--database PATH] [--duckdb PATH]
                              build every model in DuckDB, in one transaction
                              (database default: DIR/eider.duckdb;
                              duckdb default: `duckdb` on PATH)
       eider --version";

/// A command and its arguments, once parsed.
struct Invocation {
    command: String,
    root: PathBuf,
    database: Option<PathBuf>,
    duckdb: Option<PathBuf>,
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("eider: {message}\n{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match command.as_str() {
        "--version" | "-V" => {
            outln!("eider {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "--help" | "-h" | "help" => {
            outln!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "check" | "order" | "compile" | "run" => {}
        other => return usage_error(&format!("unknown command `{other}`")),
    }
    let mut inv = Invocation {
        command: command.clone(),
        root: PathBuf::from("."),
        database: None,
        duckdb: None,
    };
    let mut dir_seen = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                outln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--database" | "--duckdb" if inv.command == "run" => {
                let Some(value) = it.next() else {
                    return usage_error(&format!("`{arg}` needs a path"));
                };
                let slot = if arg == "--database" {
                    &mut inv.database
                } else {
                    &mut inv.duckdb
                };
                if slot.replace(PathBuf::from(value)).is_some() {
                    return usage_error(&format!("`{arg}` given twice"));
                }
            }
            // A leading `-` is an unknown flag, not a directory.
            flag if flag.starts_with('-') => {
                return usage_error(&format!("unknown option `{flag}`"));
            }
            dir if !dir_seen => {
                inv.root = PathBuf::from(dir);
                dir_seen = true;
            }
            _ => return usage_error("only one project directory"),
        }
    }
    execute(&inv)
}

fn execute(inv: &Invocation) -> ExitCode {
    let root = inv.root.as_path();
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
        outln!(
            "{}:{line}:{col}: {} {}",
            display(root, &d.path).display(),
            d.code,
            d.message
        );
    }
    if !report.diagnostics.is_empty() {
        return ExitCode::from(1);
    }
    // Nothing is ordered, compiled or built for a project with findings:
    // all three are only meaningful once the contract holds.
    match inv.command.as_str() {
        "order" => {
            for name in &report.order {
                outln!("{name}");
            }
        }
        "compile" => outln!("{}", run::script(&models, &report.order, false).trim_end()),
        "run" => return build(inv, root, &models, &report.order),
        _ => {}
    }
    ExitCode::SUCCESS
}

fn build(inv: &Invocation, root: &Path, models: &[project::Model], order: &[String]) -> ExitCode {
    let database = inv
        .database
        .clone()
        .unwrap_or_else(|| root.join("eider.duckdb"));
    let duckdb = inv
        .duckdb
        .clone()
        .unwrap_or_else(|| PathBuf::from("duckdb"));
    match run::run(&run::script(models, order, true), &duckdb, &database) {
        Ok(()) => ExitCode::SUCCESS,
        // A rejected model is a finding against that model, in the same
        // one-line form as every other finding.
        Err(run::RunError::Model {
            name,
            line,
            message,
        }) => {
            let path = models
                .iter()
                .find(|m| m.qualified() == name)
                .map_or_else(|| PathBuf::from(&name), |m| display(root, &m.path));
            outln!(
                "{}:{}:1: RUN001 DuckDB rejected the model: {message}",
                path.display(),
                line.unwrap_or(1)
            );
            ExitCode::from(1)
        }
        Err(run::RunError::Database(message)) => {
            eprintln!("eider: {}: {message}", database.display());
            ExitCode::from(2)
        }
        Err(run::RunError::Engine(message)) => {
            eprintln!("eider: {message}");
            ExitCode::from(2)
        }
    }
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
