# Contributing to eider

Thanks for helping. eider is early: `check`, `order`, `compile` and `run` work,
and the design is still moving.

## Before you start

Open an issue before a pull request for anything beyond a small fix: a new
rule, a change to the layer contract or naming patterns, or a new command.
Those decide what every eider project looks like, so let's agree on them
first.

## Building and testing

You need Rust 1.85 or newer, and the [DuckDB command
line](https://duckdb.org/docs/installation/) for the build tests in
`tests/run.rs`. Without it those tests skip; set `EIDER_TEST_DUCKDB` to its
path if it isn't on PATH. CI sets `EIDER_REQUIRE_DUCKDB=1` so they never
skip there. The same checks CI runs:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --release -- check examples/birds
```

CI also holds [`examples/birds`](examples/birds) to the house style: clean
under every [grebe](https://github.com/ncrzw9/grebe) rule and formatted by it.
If you touch the example, run `grebe check --select ALL examples` and
`grebe format --check examples` with the grebe version matching
`grebe-syntax` in `Cargo.lock`.

## Where things go

- `src/project.rs` finds models and reads paths; `src/config.rs` reads
  `eider.toml`; `src/sql.rs` reads dependencies from SQL; `src/check.rs`
  holds the rules and build order; `src/run.rs` writes the build script and
  runs it through DuckDB.
- A new rule gets a code in the README's rule table and an end-to-end test in
  `tests/cli.rs`: a small project that must trigger it. The shipped example
  must stay clean.
- Findings stay one line each, `path:line:col: CODE message`, and exit codes
  stay as documented: scripts depend on both.

## Pull requests

- One change per pull request, with tests.
- Commit messages: a short imperative summary line, as in `git log`.
- `unsafe` is forbidden.
- Don't bump versions; that happens at release.

## Accessibility

eider is a command-line tool, so its accessibility is its output: plain
text, no meaning carried by color alone, and messages that read sensibly
through a screen reader. If something gets in your way, open an issue.

## Code of Conduct

This project follows the [Contributor Covenant](https://github.com/ncrzw9/.github/blob/main/CODE_OF_CONDUCT.md).
Security issues go through the [security policy](https://github.com/ncrzw9/.github/blob/main/SECURITY.md), not public issues.

By contributing you agree your work is licensed under the [MIT License](LICENSE).
