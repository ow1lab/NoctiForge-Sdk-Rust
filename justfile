# Default command: List all available just commands
default:
    @just --list

# Run rustfmt (always the same)
fmt:
    cargo fmt -- --check

# Clippy with optional release flag
lint release_flag = "":
    cargo clippy {{release_flag}} --all-targets --all-features -- -D warnings

# Tests with optional release flag
test release_flag = "":
    cargo test {{release_flag}} --all

# Combined check
check release_flag = "":
    just fmt
    just lint {{release_flag}}
    just test {{release_flag}}

update:
    nix flake update
    cargo update
