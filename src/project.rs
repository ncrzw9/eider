//! Project layout: where models live and what their paths mean.
//!
//! A model's path is its declaration. `models/<layer>/<domain>/<name>.sql`
//! fixes the layer it is built into, the domain it belongs to and the name
//! it is referenced by (`<layer>.<name>`). Nothing else configures a model,
//! so there is exactly one place to look to know what a file is.

use std::path::{Path, PathBuf};

/// The layers eider builds, in dependency order.
///
/// - `staging`: views that clean, rename and cast one source relation.
/// - `silver`: the canonical model, insert-only.
/// - `gold`: output shaped for its consumers, rebuilt from silver on every
///   run.
pub const LAYERS: [&str; 3] = ["staging", "silver", "gold"];

#[derive(Debug, Clone)]
pub struct Model {
    pub layer: &'static str,
    pub domain: String,
    /// The file stem, as written. Rules check its form.
    pub name: String,
    pub path: PathBuf,
    pub sql: String,
}

impl Model {
    /// How other models refer to this one. Lowercased because identifiers
    /// compare case-insensitively in DuckDB.
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.layer, self.name.to_ascii_lowercase())
    }
}

/// A `.sql` file under `models/` that does not sit at
/// `models/<layer>/<domain>/<name>.sql`.
#[derive(Debug)]
pub struct Misplaced {
    pub path: PathBuf,
    pub reason: String,
}

/// Every `.sql` file under `<root>/models`, sorted by path so output order
/// never depends on the filesystem.
pub fn discover(root: &Path) -> std::io::Result<(Vec<Model>, Vec<Misplaced>)> {
    let models_dir = root.join("models");
    let mut files = Vec::new();
    if models_dir.is_dir() {
        walk(&models_dir, &mut files)?;
    }
    files.sort();

    let mut models = Vec::new();
    let mut misplaced = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&models_dir).unwrap_or(&path);
        let parts: Vec<String> = rel
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let layer = parts
            .first()
            .and_then(|l| LAYERS.iter().find(|x| **x == l.as_str()));
        match (parts.len(), layer) {
            (3, Some(layer)) => {
                let name = Path::new(&parts[2])
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let sql = std::fs::read_to_string(&path)?;
                models.push(Model {
                    layer,
                    domain: parts[1].clone(),
                    name,
                    path,
                    sql,
                });
            }
            (_, None) => misplaced.push(Misplaced {
                path,
                reason: format!(
                    "`{}` is not a layer; models live under {}",
                    parts.first().map_or("", String::as_str),
                    LAYERS.map(|l| format!("models/{l}/")).join(", ")
                ),
            }),
            _ => misplaced.push(Misplaced {
                path,
                reason: "models live at models/<layer>/<domain>/<name>.sql".into(),
            }),
        }
    }
    Ok((models, misplaced))
}

/// Symlinks are not followed: a link loop would otherwise recurse forever,
/// and a model must live inside the project to be part of it.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "sql") {
            out.push(path);
        }
    }
    Ok(())
}
