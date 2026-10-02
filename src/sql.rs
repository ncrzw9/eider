//! What a model's SQL reads, taken from the parse tree.
//!
//! Dependencies come from the SQL itself, not from a templating call: every
//! table reference in the tree is an edge. The parser is grebe-syntax, which
//! is generated from DuckDB's own grammar, so a model is valid DuckDB exactly
//! as written and can be run, formatted and linted without eider.

use grebe_syntax::Span;
use grebe_syntax::cst::{NodeId, Tree};
use grebe_syntax::matcher::parse;

/// A table reference, split into its dotted parts (lowercased, quotes
/// removed): `bronze.fieldbook__sightings` → `["bronze", "fieldbook__sightings"]`.
/// A string literal (`FROM 'sightings.csv'`, which DuckDB reads as a file)
/// is kept whole, quotes included, as a single part.
#[derive(Debug, Clone)]
pub struct Relation {
    pub parts: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TableFunction {
    pub name: String,
    pub span: Span,
}

#[derive(Debug)]
pub enum Analysis {
    /// The text is not valid DuckDB SQL.
    Unparsed,
    Parsed {
        /// The kind of each top-level statement (`SelectStatement`, …).
        statements: Vec<(&'static str, Span)>,
        relations: Vec<Relation>,
        /// Names introduced by `WITH`, lowercased. An unqualified reference
        /// to one of these is not a relation.
        ctes: Vec<String>,
        table_functions: Vec<TableFunction>,
    },
}

pub fn analyze(src: &str) -> Analysis {
    let Some(tree) = parse(src) else {
        return Analysis::Unparsed;
    };
    let statements = tree
        .find("Statement")
        .into_iter()
        .filter(|&s| {
            tree.parent(s)
                .is_some_and(|p| tree.rule_name(p) == "TopLevelStatement")
        })
        .map(|s| {
            let kind = tree
                .children(s)
                .first()
                .map_or("Statement", |&c| tree.rule_name(c));
            (kind, tree.node(s).span)
        })
        .collect();
    let relations = tree
        .find("BaseTableName")
        .into_iter()
        .map(|id| Relation {
            parts: split_name(tree.text(id, src)),
            span: tree.node(id).span,
        })
        .collect();
    let ctes = tree
        .find("WithStatement")
        .into_iter()
        .filter_map(|w| first_child_text(&tree, w, src))
        .map(|name| unquote(&name))
        .collect();
    let table_functions = tree
        .find("TableFunctionName")
        .into_iter()
        .map(|id| TableFunction {
            name: unquote(tree.text(id, src)),
            span: tree.node(id).span,
        })
        .collect();
    Analysis::Parsed {
        statements,
        relations,
        ctes,
        table_functions,
    }
}

fn first_child_text(tree: &Tree, id: NodeId, src: &str) -> Option<String> {
    let &first = tree.children(id).first()?;
    Some(tree.text(first, src).to_string())
}

/// Splits `a."b.c".d` on dots outside double quotes. A single-quoted
/// literal is a file path, not a dotted name, so it stays one part.
fn split_name(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.starts_with('\'') {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            '.' if !in_quotes => parts.push(unquote(&std::mem::take(&mut current))),
            c if c.is_whitespace() && !in_quotes => {}
            c => current.push(c),
        }
    }
    parts.push(unquote(&current));
    parts
}

/// Identifiers compare case-insensitively, as in DuckDB; quoting only
/// protects characters, so it is removed before comparison.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s);
    s.replace("\"\"", "\"").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Parts = (
        Vec<&'static str>,
        Vec<Vec<String>>,
        Vec<String>,
        Vec<String>,
    );

    fn parsed(src: &str) -> Parts {
        match analyze(src) {
            Analysis::Parsed {
                statements,
                relations,
                ctes,
                table_functions,
            } => (
                statements.into_iter().map(|s| s.0).collect(),
                relations.into_iter().map(|r| r.parts).collect(),
                ctes,
                table_functions.into_iter().map(|t| t.name).collect(),
            ),
            Analysis::Unparsed => panic!("did not parse: {src}"),
        }
    }

    #[test]
    fn finds_relations_ctes_and_functions() {
        let (stmts, rels, ctes, fns) = parsed(
            "WITH x AS (SELECT a FROM bronze.fieldbook__sightings) \
             SELECT * FROM x JOIN Silver.\"Birds__Hub\" h USING (a), read_parquet('f') r",
        );
        assert_eq!(stmts, ["SelectStatement"]);
        assert_eq!(
            rels,
            [
                vec!["bronze".to_string(), "fieldbook__sightings".into()],
                vec!["x".into()],
                vec!["silver".into(), "birds__hub".into()],
            ]
        );
        assert_eq!(ctes, ["x"]);
        assert_eq!(fns, ["read_parquet"]);
    }

    #[test]
    fn three_part_names_and_statement_kinds() {
        let (stmts, rels, _, _) =
            parsed("SELECT * FROM bronze.main.t; CREATE TABLE silver.y AS SELECT 1");
        assert_eq!(stmts, ["SelectStatement", "CreateStatement"]);
        assert_eq!(rels[0], ["bronze", "main", "t"]);
    }

    #[test]
    fn string_literal_relation_is_one_part() {
        let (_, rels, _, _) = parsed("SELECT * FROM 'Field.Sightings.csv'");
        assert_eq!(rels, [vec!["'Field.Sightings.csv'".to_string()]]);
    }

    #[test]
    fn invalid_sql_is_unparsed() {
        assert!(matches!(analyze("SELEC 1"), Analysis::Unparsed));
    }
}
