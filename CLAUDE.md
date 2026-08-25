# sleepless — project rules

- **README.md is a living document.** Every implementation phase ends by updating README.md to
  match reality: features, CLI flags, keybindings, install/run instructions, and behavior notes.
  A phase is not done until the README reflects it.
- A phase is not done until `cargo clippy` and `cargo test` pass clean.
- **Keep the core guarantee intact:** only process-lifetime-bound inhibition mechanisms are
  allowed (D-Bus connection-scoped cookies, logind pipe fds). Never add anything that needs
  cleanup after the process dies — closing the terminal must always restore normal sleep,
  even on SIGKILL.
