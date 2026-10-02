//! `eider check`: the layer contract, enforced offline.
//!
//! Every rule runs on the parse tree without a database, so the check is
//! fast enough for every save and needs nothing but the project files in
//! CI. Rules are errors, not warnings: the contract is what makes the
//! pipeline safe to run, so a project that breaks it does not run.
//!
//! | Code     | Rule                                                        |
//! |----------|-------------------------------------------------------------|
//! | `PRJ001` | a `.sql` file under `models/` is not at `<layer>/<domain>/` |
//! | `PRS001` | a model is not valid DuckDB SQL                             |
//! | `LAY001` | a model reads a layer it may not read                       |
//! | `LAY002` | a model is not exactly one `SELECT`                         |
//! | `LAY003` | a relation is not qualified by a known catalog              |
//! | `LAY004` | a source relation is not declared in `eider.toml`           |
//! | `LAY005` | a model reads files directly (`read_parquet`, `'x.csv'`, …) |
//! | `NAM001` | a model name does not follow its layer's pattern            |
//! | `NAM002` | two models in one layer share a name                        |
//! | `DAG001` | a model reads a model that does not exist                   |
//! | `DAG002` | models depend on each other in a cycle                      |

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use grebe_syntax::Span;

use crate::config::Config;
use crate::project::{Misplaced, Model};
use crate::sql::{Analysis, analyze};

/// A finding: a code, a file and, when the finding is about specific text,
/// its byte span. Messages are for people; nothing matches on them.
///
/// Field order is the sort order: by file, then position, then code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub span: Option<(usize, usize)>,
    pub code: &'static str,
    pub message: String,
}

pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    /// Models in build order (dependencies first), as `layer.name`. Empty
    /// whenever there are diagnostics: an order is only given for a project
    /// that passes the check.
    pub order: Vec<String>,
}

/// The raw tier, recognised by its catalog name. Only staging and silver
/// may read it: gold is built from the canonical model, never straight from
/// raw data.
const RAW_SOURCE: &str = "bronze";

/// Reading a file directly hides a dependency from the graph and ties the
/// model to one environment's storage. Sources are declared instead.
///
/// DuckDB's file-reading table functions are named `read_*` or `*_scan`;
/// the rest are listed by name. `query` and `query_table` take SQL or a
/// table name as a string, which hides the dependency from the parse tree
/// just as a file path does.
fn is_file_reader(function: &str) -> bool {
    function.starts_with("read_")
        || function.ends_with("_scan")
        || matches!(
            function,
            "glob"
                | "sniff_csv"
                | "parquet_metadata"
                | "parquet_schema"
                | "parquet_file_metadata"
                | "query"
                | "query_table"
        )
}

pub fn check(config: &Config, models: &[Model], misplaced: &[Misplaced]) -> Report {
    let mut diags: Vec<Diagnostic> = misplaced
        .iter()
        .map(|m| Diagnostic {
            path: m.path.clone(),
            span: None,
            code: "PRJ001",
            message: m.reason.clone(),
        })
        .collect();

    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    for (i, m) in models.iter().enumerate() {
        if let Some(msg) = name_problem(m) {
            diags.push(file_diag(m, "NAM001", msg));
        }
        if let Some(&first) = by_name.get(&m.qualified()) {
            diags.push(file_diag(
                m,
                "NAM002",
                format!(
                    "`{}` is already defined by {}",
                    m.qualified(),
                    models[first].path.display()
                ),
            ));
        } else {
            by_name.insert(m.qualified(), i);
        }
    }

    let analyses = analyze_all(models);
    let mut edges: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); models.len()];
    for (i, (model, analysis)) in models.iter().zip(&analyses).enumerate() {
        let Analysis::Parsed {
            statements,
            relations,
            ctes,
            table_functions,
        } = analysis
        else {
            diags.push(file_diag(model, "PRS001", "not valid DuckDB SQL".into()));
            continue;
        };
        // eider generates every statement that writes, so a model that
        // carried its own DDL or DML could bypass the layer contract.
        match statements.as_slice() {
            [("SelectStatement", _)] => {}
            [] => diags.push(file_diag(
                model,
                "LAY002",
                "a model is one SELECT; this file has none".into(),
            )),
            [(_, span)] => diags.push(span_diag(
                model,
                *span,
                "LAY002",
                "a model is one SELECT; eider writes every CREATE, INSERT and ALTER itself".into(),
            )),
            [_, (_, span), ..] => diags.push(span_diag(
                model,
                *span,
                "LAY002",
                format!(
                    "a model is one SELECT; this file has {} statements",
                    statements.len()
                ),
            )),
        }
        for f in table_functions {
            if is_file_reader(&f.name) {
                diags.push(span_diag(
                    model,
                    f.span,
                    "LAY005",
                    format!(
                        "`{}` reads files directly; declare the data as a source",
                        f.name
                    ),
                ));
            }
        }
        for rel in relations {
            // Every reference names its catalog, so each edge resolves to a
            // layer or a declared source from the SQL alone.
            let (catalog, name) = match rel.parts.as_slice() {
                [only] if only.starts_with('\'') => {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "LAY005",
                        format!("{only} is read as a file; declare the data as a source"),
                    ));
                    continue;
                }
                [only] if ctes.contains(only) => continue,
                [only] => {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "LAY003",
                        format!("`{only}` is not qualified; write `<catalog>.{only}`"),
                    ));
                    continue;
                }
                [catalog, .., name] => (catalog.as_str(), name.as_str()),
                [] => continue,
            };
            if let Some(declared) = config.sources.get(catalog) {
                if !may_read(model.layer, catalog, true) {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "LAY001",
                        read_message(model, catalog),
                    ));
                } else if !declared.contains(name) {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "LAY004",
                        format!("`{catalog}.{name}` is not declared under [sources] in eider.toml"),
                    ));
                }
            } else if crate::project::LAYERS.contains(&catalog) {
                if !may_read(model.layer, catalog, false) {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "LAY001",
                        read_message(model, catalog),
                    ));
                } else if let Some(&target) = by_name.get(&format!("{catalog}.{name}")) {
                    edges[i].insert(target);
                } else {
                    diags.push(span_diag(
                        model,
                        rel.span,
                        "DAG001",
                        format!("no model `{catalog}.{name}`"),
                    ));
                }
            } else {
                diags.push(span_diag(
                    model,
                    rel.span,
                    "LAY003",
                    format!("`{catalog}` is not a layer or a declared source catalog"),
                ));
            }
        }
    }

    // Report only the models on a cycle: models merely downstream of one
    // are not at fault.
    let order = match build_order(&edges) {
        Ok(order) => order,
        Err(cycle) => {
            let names: Vec<String> = cycle.iter().map(|&i| models[i].qualified()).collect();
            for &i in &cycle {
                diags.push(file_diag(
                    &models[i],
                    "DAG002",
                    format!("dependency cycle among {}", names.join(", ")),
                ));
            }
            Vec::new()
        }
    };
    // Sorted so the output is the same on every run, whatever the order in
    // which parallel parsing or the filesystem produced findings.
    diags.sort();
    let order = if diags.is_empty() {
        order.into_iter().map(|i| models[i].qualified()).collect()
    } else {
        Vec::new()
    };
    Report {
        diagnostics: diags,
        order,
    }
}

/// Data flows one way: sources → staging → silver → gold. Staging reads
/// only sources, so each staging view stays a thin cleanup of one input.
/// Silver may build on staging and on other silver models. Gold reads only
/// silver (plus non-raw sources such as reference tables), so every
/// output is explained by the canonical model. Nothing reads gold: it is
/// build output, regenerated at will.
fn may_read(layer: &str, catalog: &str, is_source: bool) -> bool {
    match (layer, is_source) {
        (_, true) if catalog == RAW_SOURCE => matches!(layer, "staging" | "silver"),
        (_, true) => true,
        ("staging", false) => false,
        ("silver", false) => matches!(catalog, "staging" | "silver"),
        ("gold", false) => catalog == "silver",
        _ => false,
    }
}

fn read_message(model: &Model, catalog: &str) -> String {
    format!("{} models may not read `{catalog}`", model.layer)
}

/// Each layer's naming pattern carries the model's role, so a reader knows
/// what a relation is from its name alone. Models are referenced as
/// `<layer>.<name>` without their domain, so staging and silver names repeat
/// the domain to stay unique across domains.
///
/// - staging: `stg_<domain>__<name>`
/// - silver: `<domain>__hub_<entity>`, `<domain>__lnk_<name>`, or
///   `<domain>__sat_<entity>__<system>` (one satellite per source system)
/// - gold: any lowercase snake_case name, chosen for its consumers
fn name_problem(m: &Model) -> Option<String> {
    let snake = |s: &str| {
        !s.is_empty()
            && s.starts_with(|c: char| c.is_ascii_lowercase())
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if !snake(&m.domain) {
        return Some(format!(
            "domain directory `{}` is not lowercase snake_case",
            m.domain
        ));
    }
    if !snake(&m.name) {
        return Some(format!("`{}` is not lowercase snake_case", m.name));
    }
    let d = &m.domain;
    let ok = match m.layer {
        "staging" => m
            .name
            .strip_prefix(&format!("stg_{d}__"))
            .is_some_and(|rest| !rest.is_empty()),
        "silver" => m.name.strip_prefix(&format!("{d}__")).is_some_and(|rest| {
            let entity = |s: &str| !s.is_empty() && !s.starts_with('_') && !s.ends_with('_');
            if let Some(e) = rest
                .strip_prefix("hub_")
                .or_else(|| rest.strip_prefix("lnk_"))
            {
                entity(e) && !e.contains("__")
            } else if let Some(e) = rest.strip_prefix("sat_") {
                e.split_once("__").is_some_and(|(entity_name, system)| {
                    entity(entity_name) && entity(system) && !system.contains("__")
                })
            } else {
                false
            }
        }),
        _ => true,
    };
    if ok {
        return None;
    }
    Some(match m.layer {
        "staging" => format!("staging models are named `stg_{d}__<name>`"),
        _ => format!(
            "silver models are named `{d}__hub_<entity>`, `{d}__lnk_<name>` or `{d}__sat_<entity>__<system>`"
        ),
    })
}

/// Parses every model in parallel: parsing dominates the check, and models
/// are independent until the graph is built.
fn analyze_all(models: &[Model]) -> Vec<Analysis> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = models.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = models
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || batch.iter().map(|m| analyze(&m.sql)).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("analysis thread panicked"))
            .collect()
    })
}

/// Kahn's algorithm over `edges[i]` = the models `i` reads. Ties break by
/// index, so the order is stable for a given project. On a cycle, returns
/// the models that could not be ordered.
fn build_order(edges: &[BTreeSet<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    let n = edges.len();
    let mut pending: Vec<usize> = edges.iter().map(BTreeSet::len).collect();
    let mut readers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, deps) in edges.iter().enumerate() {
        for &d in deps {
            readers[d].push(i);
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| pending[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = ready.pop_first() {
        order.push(i);
        for &r in &readers[i] {
            pending[r] -= 1;
            if pending[r] == 0 {
                ready.insert(r);
            }
        }
    }
    if order.len() == n {
        return Ok(order);
    }
    // Everything left is in a cycle or downstream of one. Peel off models
    // nothing left reads; what remains is the cycles themselves.
    let mut left: BTreeSet<usize> = (0..n).filter(|&i| pending[i] > 0).collect();
    loop {
        let leaves: Vec<usize> = left
            .iter()
            .copied()
            .filter(|&i| !readers[i].iter().any(|r| left.contains(r)))
            .collect();
        if leaves.is_empty() {
            return Err(left.into_iter().collect());
        }
        for i in leaves {
            left.remove(&i);
        }
    }
}

fn file_diag(m: &Model, code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        path: m.path.clone(),
        span: None,
        code,
        message,
    }
}

fn span_diag(m: &Model, span: Span, code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        path: m.path.clone(),
        span: Some((span.start as usize, span.end as usize)),
        code,
        message,
    }
}
