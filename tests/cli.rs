//! End-to-end: the real binary against real project directories. Each rule
//! has a project that must trigger it, and the shipped example must pass.

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

/// A throwaway project: `eider.toml` plus `(relative path, contents)` files.
struct Project(PathBuf);

impl Project {
    fn new(tag: &str, toml: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "eider-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("eider.toml"), toml).unwrap();
        for (rel, body) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        Project(dir)
    }

    fn check(&self) -> (i32, String) {
        let (code, out, err) = eider(&["check", self.0.to_str().unwrap()]);
        (code, out + &err)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SOURCES: &str = "[sources]\nbronze = [\"raw_sighting\"]\nreference = [\"birds__taxonomy\"]\n";
const STG: (&str, &str) = (
    "models/staging/birds/stg_birds__sighting.sql",
    "SELECT species AS species_code FROM bronze.raw_sighting",
);
const HUB: (&str, &str) = (
    "models/silver/birds/birds__hub_species.sql",
    "SELECT DISTINCT sha256(species_code) AS hk, species_code AS business_key, 'birds' AS domain \
     FROM staging.stg_birds__sighting",
);

/// Asserts the project fails with exactly the given code on the given file.
fn expect(tag: &str, files: &[(&str, &str)], code: &str, file: &str) {
    let p = Project::new(tag, SOURCES, files);
    let (status, out) = p.check();
    assert_eq!(status, 1, "{tag}: expected findings, got:\n{out}");
    let hit = out
        .lines()
        .any(|l| l.starts_with(file) && l.contains(&format!(" {code} ")));
    assert!(hit, "{tag}: expected {code} on {file}, got:\n{out}");
}

#[test]
fn example_project_is_clean_and_ordered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/birds");
    let (code, out, err) = eider(&["check", root.to_str().unwrap()]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));

    let (code, out, _) = eider(&["order", root.to_str().unwrap()]);
    assert_eq!(code, 0);
    let order: Vec<&str> = out.lines().collect();
    let pos = |m: &str| order.iter().position(|x| *x == m).unwrap();
    assert!(pos("staging.stg_birds__fieldbook_species") < pos("silver.birds__hub_species"));
    assert!(pos("silver.birds__sat_sighting__fieldbook") < pos("gold.species_tallies"));
    assert_eq!(order.len(), 6);
}

#[test]
fn prj001_misplaced_file() {
    expect(
        "prj001",
        &[STG, ("models/birds/x.sql", "SELECT 1")],
        "PRJ001",
        "models/birds/x.sql",
    );
    expect(
        "prj001b",
        &[("models/silver/x.sql", "SELECT 1")],
        "PRJ001",
        "models/silver/x.sql",
    );
}

#[test]
fn prs001_invalid_sql() {
    expect(
        "prs001",
        &[(STG.0, "SELEC species FROM bronze.raw_sighting")],
        "PRS001",
        STG.0,
    );
}

#[test]
fn lay001_read_direction() {
    let gold = (
        "models/gold/atlas/tallies.sql",
        "SELECT * FROM silver.birds__hub_species",
    );
    // Gold may not read raw data, nor may anything read gold.
    expect(
        "lay001a",
        &[
            STG,
            HUB,
            (
                "models/gold/atlas/raw.sql",
                "SELECT * FROM bronze.raw_sighting",
            ),
        ],
        "LAY001",
        "models/gold/atlas/raw.sql",
    );
    expect(
        "lay001b",
        &[
            STG,
            HUB,
            gold,
            (
                "models/silver/birds/birds__hub_x.sql",
                "SELECT * FROM gold.tallies",
            ),
        ],
        "LAY001",
        "models/silver/birds/birds__hub_x.sql",
    );
    // Staging reads only sources.
    expect(
        "lay001c",
        &[
            STG,
            HUB,
            (
                "models/staging/birds/stg_birds__y.sql",
                "SELECT * FROM silver.birds__hub_species",
            ),
        ],
        "LAY001",
        "models/staging/birds/stg_birds__y.sql",
    );
}

#[test]
fn lay002_one_select() {
    expect(
        "lay002a",
        &[(STG.0, "CREATE TABLE staging.x AS SELECT 1")],
        "LAY002",
        STG.0,
    );
    expect(
        "lay002b",
        &[(STG.0, "SELECT species FROM bronze.raw_sighting; SELECT 1")],
        "LAY002",
        STG.0,
    );
    expect("lay002c", &[(STG.0, "-- nothing here\n")], "LAY002", STG.0);
}

#[test]
fn lay003_qualified_by_known_catalog() {
    expect(
        "lay003a",
        &[(STG.0, "SELECT * FROM raw_sighting")],
        "LAY003",
        STG.0,
    );
    expect(
        "lay003b",
        &[(STG.0, "SELECT * FROM lake.raw_sighting")],
        "LAY003",
        STG.0,
    );
}

#[test]
fn lay004_undeclared_source() {
    expect(
        "lay004",
        &[(STG.0, "SELECT * FROM bronze.raw_nest")],
        "LAY004",
        STG.0,
    );
}

#[test]
fn lay005_no_direct_file_reads() {
    expect(
        "lay005",
        &[(STG.0, "SELECT * FROM read_parquet('s3://lake/x/*.parquet')")],
        "LAY005",
        STG.0,
    );
}

#[test]
fn lay005_string_literal_is_a_file_read() {
    expect(
        "lay005b",
        &[(STG.0, "SELECT * FROM 'sightings.csv'")],
        "LAY005",
        STG.0,
    );
}

#[test]
fn nam001_layer_naming() {
    expect(
        "nam001a",
        &[(
            "models/staging/birds/sighting_clean.sql",
            "SELECT * FROM bronze.raw_sighting",
        )],
        "NAM001",
        "models/staging/birds/sighting_clean.sql",
    );
    expect(
        "nam001b",
        &[
            STG,
            (
                "models/silver/birds/birds__species.sql",
                "SELECT * FROM staging.stg_birds__sighting",
            ),
        ],
        "NAM001",
        "models/silver/birds/birds__species.sql",
    );
    // A satellite names its source system.
    expect(
        "nam001c",
        &[
            STG,
            (
                "models/silver/birds/birds__sat_species.sql",
                "SELECT * FROM staging.stg_birds__sighting",
            ),
        ],
        "NAM001",
        "models/silver/birds/birds__sat_species.sql",
    );
    // The domain prefix must match the directory.
    expect(
        "nam001d",
        &[
            STG,
            (
                "models/silver/birds/fish__hub_trout.sql",
                "SELECT * FROM staging.stg_birds__sighting",
            ),
        ],
        "NAM001",
        "models/silver/birds/fish__hub_trout.sql",
    );
}

#[test]
fn nam002_duplicate_name_in_layer() {
    expect(
        "nam002",
        &[
            ("models/gold/a/checklist.sql", "SELECT 1"),
            ("models/gold/b/checklist.sql", "SELECT 2"),
        ],
        "NAM002",
        "models/gold/b/checklist.sql",
    );
}

#[test]
fn dag001_unknown_model() {
    expect(
        "dag001",
        &[(
            "models/gold/atlas/x.sql",
            "SELECT * FROM silver.birds__hub_missing",
        )],
        "DAG001",
        "models/gold/atlas/x.sql",
    );
}

#[test]
fn dag002_cycle_names_only_cycle_members() {
    let p = Project::new(
        "dag002",
        SOURCES,
        &[
            (
                "models/silver/birds/birds__hub_a.sql",
                "SELECT * FROM silver.birds__hub_b",
            ),
            (
                "models/silver/birds/birds__hub_b.sql",
                "SELECT * FROM silver.birds__hub_a",
            ),
            (
                "models/gold/atlas/downstream.sql",
                "SELECT * FROM silver.birds__hub_a",
            ),
        ],
    );
    let (status, out) = p.check();
    assert_eq!(status, 1);
    let cycle: Vec<&str> = out.lines().filter(|l| l.contains(" DAG002 ")).collect();
    assert_eq!(cycle.len(), 2, "{out}");
    assert!(!out.contains("downstream.sql"), "{out}");
}

#[test]
fn ctes_are_not_relations() {
    let p = Project::new(
        "cte",
        SOURCES,
        &[(
            STG.0,
            "WITH a AS (SELECT * FROM bronze.raw_sighting) SELECT * FROM a",
        )],
    );
    assert_eq!(p.check(), (0, String::new()));
}

#[test]
fn project_errors_exit_2() {
    let p = Project::new("cfg", "[models]\n", &[]);
    let (status, out) = p.check();
    assert_eq!(status, 2, "{out}");
    let (status, _, _) = eider(&["check", "/definitely/not/here"]);
    assert_eq!(status, 2);
    let (status, _, _) = eider(&["frobnicate"]);
    assert_eq!(status, 2);
}

#[test]
fn findings_carry_line_and_column() {
    let p = Project::new(
        "span",
        SOURCES,
        &[(STG.0, "SELECT *\nFROM   bronze.raw_nest")],
    );
    let (_, out) = p.check();
    assert!(out.contains(&format!("{}:2:8: LAY004", STG.0)), "{out}");
}

#[test]
fn lay005_query_functions_hide_dependencies() {
    expect(
        "lay005q",
        &[(STG.0, "SELECT * FROM query_table('bronze.raw_sighting')")],
        "LAY005",
        STG.0,
    );
}

#[cfg(unix)]
#[test]
fn symlink_loops_are_not_followed() {
    let p = Project::new("symlink", SOURCES, &[STG]);
    let dir = p.0.join("models/staging/birds");
    std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
    assert_eq!(p.check(), (0, String::new()));
}

#[test]
fn subcommand_help_prints_usage() {
    let (status, out, _) = eider(&["check", "--help"]);
    assert_eq!(status, 0);
    assert!(out.contains("eider check"), "{out}");
}

#[test]
fn a_closed_stdout_ends_the_run_quietly() {
    // Enough models that `order` writes more than a pipe buffer holds.
    let files: Vec<(String, String)> = (0..6000)
        .map(|i| {
            (
                format!("models/gold/atlas/output_with_a_long_name_{i:05}.sql"),
                "SELECT 1".to_string(),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let p = Project::new("pipe", SOURCES, &refs);
    let mut child = Command::new(env!("CARGO_BIN_EXE_eider"))
        .args(["order", p.0.to_str().unwrap()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.code(), Some(141), "{stderr}");
}
