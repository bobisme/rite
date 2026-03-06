# Testing

Integration tests use **isolated temp directories** to avoid touching real data (`~/.local/share/rite/`).

## Key Mechanism

Set `RITE_DATA_DIR` to a temp directory. All rite commands will use that directory instead of the default location.

## Test Harness (`tests/common/mod.rs`)

- `TestProject::new()` — creates a temp dir, sets up `data/` and `project/` subdirectories
- `project.agent("name")` — returns an `Agent` handle that runs commands with `RITE_DATA_DIR` and `RITE_AGENT` set
- `agent.run(&["send", "general", "hello"])` — runs any rite command in isolation
- `RiteOutput` — wraps stdout/stderr with `.assert_success()`, `.stdout_contains()`, etc.

## Example: Two-Machine Sync

```rust
#[test]
fn test_two_machine_sync() {
    let machine_a = TestProject::new();
    let machine_b = TestProject::new();
    let bare_repo = TempDir::new().unwrap();

    // Create bare git repo as "remote"
    Command::new("git").args(["init", "--bare"]).arg(bare_repo.path()).status().unwrap();

    // Init sync on machine A, add remote, push
    let agent_a = machine_a.agent("agent-a");
    agent_a.run(&["sync", "init"]).assert_success();
    // ... git remote add, push, etc.

    // Machine B clones, pulls, sees A's messages
    // Machine B sends, pushes
    // Machine A pulls, sees all messages
}
```

## Running Tests

```bash
cargo test --all-features          # All tests
cargo test --test sync             # Just sync tests
cargo test --test multi_agent      # Just multi-agent tests
```

Test files: `tests/sync.rs`, `tests/multi_agent.rs`, `tests/claims.rs`, `tests/labels.rs`, `tests/read_tracking.rs`, `tests/tui.rs`
