//! Building a project: the SQL eider writes, and running it through DuckDB.
//!
//! eider writes every statement that changes the warehouse, and those
//! statements are few: one schema per layer, then one `CREATE OR REPLACE`
//! per model in build order. Staging models become views, because they only
//! clean and rename one source relation. Silver and gold models become
//! tables, rebuilt in full on every run.
//!
//! The whole build is one transaction. If DuckDB rejects any model, nothing
//! the run did is kept, so a database is always the result of one complete
//! build.
//!
//! eider does not link DuckDB. `run` hands the script to the `duckdb` command
//! line the user already has, the same one that would run the models by
//! hand, so eider stays one small static binary and the engine version is
//! the user's choice.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::project::{LAYERS, Model};

/// What a model in each layer is built as.
fn materialization(layer: &str) -> &'static str {
    if layer == "staging" { "VIEW" } else { "TABLE" }
}

/// The model's SELECT without its terminating semicolon. LAY002 guarantees
/// the file is exactly one SELECT, so the only `;` that can end it is the
/// last one. Anything after it (a trailing comment) stays, and the statement
/// is terminated on a line of its own so a trailing `--` comment cannot
/// swallow the semicolon.
fn body(sql: &str) -> &str {
    let t = sql.trim_end();
    t.strip_suffix(';').unwrap_or(t).trim_end()
}

/// The build script. With `progress`, each model is preceded by a DuckDB
/// shell `.print` line naming it, so a failed run can say which model the
/// engine rejected; without it, the script is plain SQL.
pub fn script(models: &[Model], order: &[String], progress: bool) -> String {
    let mut out = String::new();
    out.push_str(
        "-- Written by eider. Every statement here is eider's; the models are the SELECTs.\n",
    );
    out.push_str("BEGIN TRANSACTION;\n");
    for layer in LAYERS {
        out.push_str(&format!("CREATE SCHEMA IF NOT EXISTS {layer};\n"));
    }
    for name in order {
        let Some(m) = models.iter().find(|m| &m.qualified() == name) else {
            continue;
        };
        out.push('\n');
        if progress {
            out.push_str(&format!(".print eider: building {name}\n"));
        }
        out.push_str(&format!(
            "CREATE OR REPLACE {} {name} AS\n{}\n;\n",
            materialization(m.layer),
            body(&m.sql)
        ));
    }
    out.push_str("\nCOMMIT;\n");
    out
}

/// Why a run did not complete.
#[derive(Debug)]
pub enum RunError {
    /// The `duckdb` program could not be started or talked to.
    Engine(String),
    /// DuckDB rejected a model. `line` is the line within the model file,
    /// when DuckDB said where.
    Model {
        name: String,
        line: Option<usize>,
        message: String,
    },
    /// DuckDB failed outside any model: opening the database, creating a
    /// schema, or committing.
    Database(String),
}

/// Run `script` (built with `progress`) against `database` with the given
/// `duckdb` executable.
pub fn run(script: &str, duckdb: &Path, database: &Path) -> Result<(), RunError> {
    let mut child = Command::new(duckdb)
        .arg("-bail")
        .arg("-batch")
        .arg(database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            RunError::Engine(format!(
                "cannot run `{}`: {e} (install the DuckDB command line, or pass --duckdb PATH)",
                duckdb.display()
            ))
        })?;
    // Write from another thread so a chatty engine filling its output pipe
    // cannot deadlock against us filling its input.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = script.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child
        .wait_with_output()
        .map_err(|e| RunError::Engine(format!("`{}` failed: {e}", duckdb.display())))?;
    // A write error only means DuckDB stopped reading early, which it does
    // when it bails; its exit status and stderr say why.
    let _ = writer.join();
    if out.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("DuckDB exited with an error and said nothing")
        .to_string();
    match stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("eider: building "))
    {
        Some(name) => Err(RunError::Model {
            name: name.trim().to_string(),
            line: error_line(&stderr),
            message,
        }),
        None => Err(RunError::Database(message)),
    }
}

/// DuckDB reports `LINE n:` relative to the statement, whose first line is
/// eider's `CREATE OR REPLACE ... AS`; the model's line is one less.
fn error_line(stderr: &str) -> Option<usize> {
    stderr.lines().find_map(|l| {
        let n: usize = l
            .trim()
            .strip_prefix("LINE ")?
            .split(':')
            .next()?
            .parse()
            .ok()?;
        n.checked_sub(1).filter(|n| *n > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn model(layer: &'static str, name: &str, sql: &str) -> Model {
        Model {
            layer,
            domain: "d".into(),
            name: name.into(),
            path: PathBuf::from(format!("models/{layer}/d/{name}.sql")),
            sql: sql.into(),
        }
    }

    #[test]
    fn views_for_staging_tables_after_in_build_order() {
        let models = vec![
            model(
                "silver",
                "d__hub_x",
                "SELECT 1 AS hk FROM staging.stg_d__x;\n",
            ),
            model(
                "staging",
                "stg_d__x",
                "SELECT 1 AS a FROM bronze.x -- note\n",
            ),
        ];
        let order = vec![
            "staging.stg_d__x".to_string(),
            "silver.d__hub_x".to_string(),
        ];
        let s = script(&models, &order, false);
        let view = s.find(
            "CREATE OR REPLACE VIEW staging.stg_d__x AS\nSELECT 1 AS a FROM bronze.x -- note\n;\n",
        );
        let table = s.find(
            "CREATE OR REPLACE TABLE silver.d__hub_x AS\nSELECT 1 AS hk FROM staging.stg_d__x\n;\n",
        );
        assert!(view.is_some() && table.is_some() && view < table, "{s}");
        assert!(s.starts_with("-- Written by eider"));
        assert!(s.contains("BEGIN TRANSACTION;\nCREATE SCHEMA IF NOT EXISTS staging;"));
        assert!(s.trim_end().ends_with("COMMIT;"));
        assert!(!s.contains(".print"));
        assert!(script(&models, &order, true).contains(".print eider: building silver.d__hub_x\n"));
    }

    #[test]
    fn error_line_is_relative_to_the_model() {
        let stderr =
            "Binder Error: Referenced column \"x\" not found\n\nLINE 3: SELECT x\n        ^\n";
        assert_eq!(error_line(stderr), Some(2));
        assert_eq!(error_line("Catalog Error: no line here"), None);
        assert_eq!(error_line("LINE 1: CREATE OR REPLACE"), None);
    }
}
