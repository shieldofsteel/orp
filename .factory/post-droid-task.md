# Post-Droid Audit: CLI Sync + Power Audit

After the droid finishes building the CLI and tests, this agent must:

1. Read the ACTUAL CLI code: `orp-core/src/cli/args.rs` and `commands.rs`
2. Read the CLI docs: `docs/CLI_REFERENCE.md`
3. Compare every command, flag, and output format between code and docs
4. Fix CLI_REFERENCE.md to match what was ACTUALLY built
5. Check if the CLI is truly world-class — compare against gh CLI, kubectl, docker CLI
6. Verify every example in the docs actually works
7. Check shell completions are generated
8. Ensure --json/--csv/--table output modes exist
9. Verify NO_COLOR is respected
10. Add any commands the droid built that docs missed
11. Remove any docs for commands that don't exist in code
