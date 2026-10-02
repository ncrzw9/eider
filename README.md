# eider

A declarative pipeline compiler for DuckDB. Models are plain `SELECT` files;
eider owns every statement that writes to the warehouse, so the layer
contract holds by construction.

- **No templating.** Dependencies are read from the SQL itself. A model is
  valid DuckDB as written, so any DuckDB tool can run, format or lint it.
- **One static binary**, no runtime dependencies. `eider check` runs offline,
  so CI needs no database and no credentials.
- **Medallion layers with a Data Vault silver tier.** Data flows one way:
  sources → staging → silver → gold.

> Early. `eider check` and `eider order` work today; compiling and running
> models against DuckDB comes next.

## Install

```sh
cargo install eider
```

## A project

```
eider.toml
models/
  staging/birds/stg_birds__fieldbook_species.sql
  silver/birds/birds__hub_species.sql
  silver/birds/birds__sat_species__fieldbook.sql
  gold/atlas/species_tallies.sql
```

A model's path is its declaration: `models/<layer>/<domain>/<name>.sql`,
referenced elsewhere as `<layer>.<name>`. `eider.toml` declares the source
relations eider reads but never writes:

```toml
[sources]
bronze   = ["fieldbook__species"]
reference = ["birds__taxonomy"]
```

See [`examples/birds`](examples/birds) for a complete project: a birding
atlas built from a field-notes app's sightings. It is written in the house
style, clean under every [grebe](https://github.com/ncrzw9/grebe) rule and
formatted by it.

## Usage

```
eider check [DIR]    check the project in DIR (default: .)
eider order [DIR]    print models in build order
eider --version
```

Each finding is one line, `path:line:col: CODE message`, sorted by file and
position. `order` prints nothing while there are findings.

Exit codes: `0` clean, `1` findings, `2` usage or project error.

## Rules

| Code     | Rule                                                        |
|----------|-------------------------------------------------------------|
| `PRJ001` | a `.sql` file under `models/` is not at `<layer>/<domain>/` |
| `PRS001` | a model is not valid DuckDB SQL                             |
| `LAY001` | a model reads a layer it may not read                       |
| `LAY002` | a model is not exactly one `SELECT`                         |
| `LAY003` | a relation is not qualified by a known catalog              |
| `LAY004` | a source relation is not declared in `eider.toml`           |
| `LAY005` | a model reads files directly (`read_parquet`, `'x.csv'`, …) |
| `NAM001` | a model name does not follow its layer's pattern            |
| `NAM002` | two models in one layer share a name                        |
| `DAG001` | a model reads a model that does not exist                   |
| `DAG002` | models depend on each other in a cycle                      |

Who may read what: staging reads sources; silver reads sources, staging and
silver; gold reads silver and non-raw sources (any source catalog other
than `bronze`). Nothing reads gold.

Naming: staging `stg_<domain>__<name>`; silver `<domain>__hub_<entity>`,
`<domain>__lnk_<name>` or `<domain>__sat_<entity>__<system>`; gold any
snake_case name.

## License

MIT
