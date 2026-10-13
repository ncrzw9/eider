//! End-to-end for `compile` and `run`.
//!
//! The process plumbing is tested against a stand-in `duckdb` script, so it
//! runs everywhere. The real build is tested against the DuckDB command line
//! when one is installed: set `EIDER_TEST_DUCKDB` to its path, or have
//! `duckdb` on PATH. CI sets `EIDER_REQUIRE_DUCKDB=1`, which turns a missing
//! engine into a failure instead of a skip.

use std::path::{Path, PathBuf};
use std::process::Command;

fn eider(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_eider"))
        .args(args)
        .output()
        .expect("run eider");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/birds")
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "eider-run-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_str().unwrap().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copy the example project so a test can change a model.
fn copy_example(to: &Path) {
    for entry in walk(&example()) {
        let rel = entry.strip_prefix(example()).unwrap();
        let dest = to.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&entry, dest).unwrap();
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn compile_prints_one_transaction_in_build_order() {
    let (code, out, err) = eider(&["compile", example().to_str().unwrap()]);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.starts_with("-- Written by eider"), "{out}");
    let pos = |s: &str| {
        out.find(s)
            .unwrap_or_else(|| panic!("missing `{s}` in:\n{out}"))
    };
    assert!(pos("BEGIN TRANSACTION;") < pos("CREATE SCHEMA IF NOT EXISTS staging;"));
    assert!(
        pos("CREATE OR REPLACE VIEW staging.stg_birds__fieldbook_species AS")
            < pos("CREATE OR REPLACE TABLE silver.birds__hub_species AS")
    );
    assert!(pos("CREATE OR REPLACE TABLE gold.species_tallies AS") < pos("COMMIT;"));
    assert!(!out.contains(".print"), "compile output must be plain SQL");
}

#[test]
fn nothing_compiles_or_runs_while_there_are_findings() {
    let s = Scratch::new("findings");
    copy_example(&s.0);
    std::fs::write(
        s.0.join("models/gold/atlas/raw.sql"),
        "SELECT * FROM bronze.fieldbook__species",
    )
    .unwrap();
    for cmd in ["compile", "run"] {
        let (code, out, _) = eider(&[cmd, &s.path("")]);
        assert_eq!(code, 1, "{cmd}: {out}");
        assert!(out.contains("LAY001"), "{cmd}: {out}");
        assert!(!out.contains("CREATE"), "{cmd}: {out}");
    }
    assert!(!s.0.join("eider.duckdb").exists());
}

#[test]
fn run_options_belong_to_run() {
    let (code, _, err) = eider(&["check", "--database", "x.duckdb"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = eider(&["run", "--database"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn a_missing_engine_is_a_usage_error() {
    let (code, out, err) = eider(&[
        "run",
        example().to_str().unwrap(),
        "--duckdb",
        "/no/such/duckdb",
        "--database",
        "/no/such/db",
    ]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("install the DuckDB command line"), "{err}");
}

/// A stand-in `duckdb` that records its arguments and script, then plays
/// back the given stdout, stderr and exit status.
#[cfg(unix)]
fn fake_duckdb(dir: &Scratch, stdout: &str, stderr: &str, status: i32) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path("duckdb");
    let script = format!(
        "#!/bin/sh\necho \"$@\" > '{args}'\ncat > '{input}'\nprintf '%s' '{stdout}'\nprintf '%s' '{stderr}' >&2\nexit {status}\n",
        args = dir.path("args"),
        input = dir.path("input.sql"),
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn run_hands_the_script_to_duckdb_in_batch_mode() {
    let s = Scratch::new("fake-ok");
    let duckdb = fake_duckdb(&s, "", "", 0);
    let db = s.path("w.duckdb");
    let (code, out, err) = eider(&[
        "run",
        example().to_str().unwrap(),
        "--duckdb",
        &duckdb,
        "--database",
        &db,
    ]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
    let args = std::fs::read_to_string(s.0.join("args")).unwrap();
    assert_eq!(args.trim(), format!("-bail -batch {db}"));
    let input = std::fs::read_to_string(s.0.join("input.sql")).unwrap();
    assert!(input.contains(".print eider: building gold.species_tallies\nCREATE OR REPLACE TABLE gold.species_tallies AS"));
}

#[cfg(unix)]
#[test]
fn a_rejected_model_is_reported_as_a_finding_on_its_file() {
    let s = Scratch::new("fake-fail");
    let duckdb = fake_duckdb(
        &s,
        "eider: building staging.stg_birds__fieldbook_species\neider: building silver.birds__hub_species\n",
        "Binder Error: no such column\n\nLINE 2: FROM x\n",
        1,
    );
    let (code, out, err) = eider(&[
        "run",
        example().to_str().unwrap(),
        "--duckdb",
        &duckdb,
        "--database",
        &s.path("w.duckdb"),
    ]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        out.trim(),
        "models/silver/birds/birds__hub_species.sql:1:1: RUN001 DuckDB rejected the model: Binder Error: no such column"
    );
}

#[cfg(unix)]
#[test]
fn a_failure_outside_any_model_is_a_database_error() {
    let s = Scratch::new("fake-db");
    let duckdb = fake_duckdb(&s, "", "IO Error: Cannot open file\n", 1);
    let (code, out, err) = eider(&[
        "run",
        example().to_str().unwrap(),
        "--duckdb",
        &duckdb,
        "--database",
        &s.path("w.duckdb"),
    ]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("IO Error: Cannot open file"), "{err}");
}

/// The real DuckDB command line, or None to skip.
fn real_duckdb() -> Option<String> {
    let candidate = std::env::var("EIDER_TEST_DUCKDB").unwrap_or_else(|_| "duckdb".into());
    let found = Command::new(&candidate)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if found {
        return Some(candidate);
    }
    assert!(
        std::env::var_os("EIDER_REQUIRE_DUCKDB").is_none(),
        "EIDER_REQUIRE_DUCKDB is set but `{candidate}` does not run"
    );
    eprintln!("skipping: no DuckDB command line (set EIDER_TEST_DUCKDB)");
    None
}

fn sql(duckdb: &str, db: &str, query: &str) -> String {
    let out = Command::new(duckdb)
        .args(["-csv", "-noheader", db, "-c", query])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const SEED: &str = "
CREATE SCHEMA bronze; CREATE SCHEMA reference;
CREATE TABLE bronze.fieldbook__species AS SELECT * FROM (VALUES
    (' amro ', 'American Robin', 'f1', 'h1'), ('blja', 'Blue Jay', 'f1', 'h2')
) t(species_code, common_name, _source_file, _record_hash);
CREATE TABLE bronze.fieldbook__sightings AS SELECT * FROM (VALUES
    ('amro', '3', TIMESTAMP '2026-05-01 07:00', 'north', 'f2', 'h3'),
    ('amro', 'x', TIMESTAMP '2026-05-02 07:00', 'north', 'f2', 'h4'),
    ('blja', '2', TIMESTAMP '2026-05-02 08:00', 'south', 'f2', 'h5')
) t(species_code, bird_count, seen_at, region, _source_file, _record_hash);
CREATE TABLE reference.birds__taxonomy AS SELECT * FROM (VALUES ('blja', 'BLJA')) t(species_code, accepted_code);
";

#[test]
fn the_example_builds_in_duckdb_and_a_failed_build_keeps_nothing() {
    let Some(duckdb) = real_duckdb() else { return };
    let s = Scratch::new("real");
    copy_example(&s.0);
    let db = s.path("birds.duckdb");
    sql(&duckdb, &db, SEED);

    let (code, out, err) = eider(&["run", &s.path(""), "--duckdb", &duckdb, "--database", &db]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
    assert_eq!(
        sql(
            &duckdb,
            &db,
            "SELECT species, region, birds_seen FROM gold.species_tallies ORDER BY ALL"
        ),
        "BLJA,south,2\namro,north,3"
    );
    assert_eq!(
        sql(
            &duckdb,
            &db,
            "SELECT table_type FROM information_schema.tables WHERE table_name = 'stg_birds__fieldbook_species'"
        ),
        "VIEW"
    );

    // Break a silver model on its third line, and empty the gold table so
    // a rolled-back build is visible.
    std::fs::write(
        s.0.join("models/silver/birds/birds__sat_species__fieldbook.sql"),
        "SELECT\n    hk: sha256(species_code),\n    nope\nFROM staging.stg_birds__fieldbook_species;\n",
    )
    .unwrap();
    sql(&duckdb, &db, "DELETE FROM gold.species_tallies");
    let (code, out, _) = eider(&["run", &s.path(""), "--duckdb", &duckdb, "--database", &db]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.starts_with("models/silver/birds/birds__sat_species__fieldbook.sql:3:1: RUN001 DuckDB rejected the model: Binder Error"),
        "{out}"
    );
    assert_eq!(
        sql(&duckdb, &db, "SELECT count(*) FROM gold.species_tallies"),
        "0"
    );
}
