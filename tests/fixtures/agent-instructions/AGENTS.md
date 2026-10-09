# Agent instructions

These instructions are for coding agents that work in this repository.
Read the [source guide](src/README.md) before you change code in `src/`.

<!-- memoria:section id="commands" files="justfile Cargo.toml" guidance="docs/templates/agent-commands.md" -->
## Commands

- Run `just build` from the repository root to compile the project. It succeeds when Cargo prints `Finished`.
- Run `just test` from the repository root before each commit. It runs `cargo test`, and it succeeds when every test passes.
<!-- /memoria:section -->

<!-- memoria:section id="boundaries" guidance="docs/templates/agent-rules.md" -->
## Boundaries

- Do not edit `memoria.lock` by hand. Memoria writes it when a reviewer acknowledges a review. Run `memoria ack` instead.
- Do not push to `main`. Changes are reviewed in pull requests. Open a pull request instead.
<!-- /memoria:section -->
