# Security Policy

## Reporting

If you believe you have found a security issue in `primeval`, report it privately by email to [ale.burato@icloud.com](mailto:ale.burato@icloud.com).

Do not open a public GitHub issue for an unpatched vulnerability.

## What To Include

Please include as much of the following as you can:

- affected package or crate version
- operating system, architecture, and Node version if relevant
- a clear description of impact
- reproduction steps or a minimal proof of concept
- whether the issue depends on a crafted input image, package install path, or native loading behavior

## Scope

This policy covers:

- the npm package `@domoarigatomrburato/primeval`
- published native addons and their loading path
- the bundled CLI
- the Rust workspace crates in this repository

## Response Expectations

- reports will be reviewed privately first
- fixes will target the latest supported release line
- coordinated disclosure is preferred after a fix or mitigation is ready

For general usage questions or bug reports that are not security-sensitive, use the normal repository support path described in [SUPPORT.md](SUPPORT.md).