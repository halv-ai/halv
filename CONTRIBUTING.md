# Contributing to Halv

Pull requests are welcome.
For a large change, open an issue first, so that we can agree on the approach.

## Workflow

The repository uses one branch: `main`.

1. Fork the repository.
2. Make your change on a branch of your fork.
3. Open a pull request against `main`.

The maintainer merges approved pull requests into `main`.
The repository has no release branches.

The maintainer develops Halv in a private repository.
The private repository also holds the account server, the relay, and the phone app.
`scripts/export-public.mjs` copies the public part of the private repository into this repository.
The maintainer also applies each merged pull request to the private repository, so that the next export keeps it.

## Before you open a pull request

Run the tests:

```sh
pnpm test
cd src-tauri && cargo test -- --skip live_
```

For a change to `crates/halv-remote`, also run:

```sh
cd crates/halv-remote && cargo test --all-features
```

Also run `cargo fmt --all -- --check` and `cargo clippy --all-targets --all-features -- -D warnings` in `src-tauri`.
CI runs the same checks.
README.md tells you how to build the app from source.

## Writing style

Write all prose in ASD-STE100 (Simplified Technical English).
This rule applies to docs, code comments, commit messages, and pull request text.

- Write short sentences: 20 words or fewer in procedures, 25 words or fewer in descriptions.
- Write one instruction in each sentence.
- Use the active voice.
- Use one term for one concept.

UI strings keep the product voice and the rules in `docs/i18n.md`.

## Commits

- Write a short commit subject in the imperative mood, for example "Fix the tab order".
- Do not add `Co-Authored-By` trailers to commits or pull requests.

## License of contributions

You do not sign a contributor license agreement (CLA).
Halv uses the Apache License, Version 2.0.
Section 5 of that license applies: you submit your contribution under the same license ("inbound = outbound").
Submit only code that you have the right to submit.
