# fastown

`fastown` is a small Rust MVP meant to explore a parallel alternative to `chown -R` on Linux when traversing large trees with millions of files.

The current version intentionally keeps the scope small and robust:

- CLI with `-j/--jobs` and `--dry-run`
- lookup of UID/GID via libc
- tree walk with `walkdir`
- concurrent processing via `rayon`
- direct kernel calls with `fchownat`
- summary output with runtime and counters

## Build

```bash
cargo build --release
```

## Example usage

```bash
sudo ./target/release/fastown -j auto myuser:mygroup /media/usuario/DISCO
sudo ./target/release/fastown --dry-run -j 8 myuser:mygroup /media/usuario/DISCO
```

## Notes

This is an MVP, not a production-grade replacement for `chown -R`.

The most important design decisions for the next iterations are:

- keep the worker pool bounded
- avoid process spawning per file
- preserve careful symlink semantics
- never assume more threads always improve throughput
- benchmark against GNU `chown -R` on the same tree

## Why this shape

The goal is to remove the expensive part of the shell pipeline:

- do not invoke `chown` once per file
- do not rely on shell loops
- do direct path-by-path ownership changes in Rust
- parallelize only the parts that actually benefit from concurrency
