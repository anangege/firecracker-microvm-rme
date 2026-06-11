# Firecracker AGENTS.md

Compact guidance for AI agents working in the Firecracker microVM VMM repository.

## Build Commands

**Use `tools/devtool` - all builds happen inside Docker container.**

```bash
# Debug build (default)
tools/devtool build

# Release build
tools/devtool build --release

# Build for specific libc (default is musl)
tools/devtool build -l gnu

# Build from specific git revision
tools/devtool build --rev <revision> --release
```

**Binary output paths** (non-obvious):
- Debug: `build/cargo_target/${ARCH}-unknown-linux-musl/debug/firecracker`
- Release: `build/cargo_target/${ARCH}-unknown-linux-musl/release/firecracker`
- Same pattern for `jailer` binary

**Jailer is excluded from default cargo build** - requires explicit `cargo build -p jailer` or use devtool.

## Test Commands

**Integration tests** (pytest-based, run inside Docker):
```bash
# Run all PR CI tests
tools/devtool -y test

# Run specific test file (path relative to tests/)
tools/devtool -y test -- integration_tests/functional/test_api.py

# Run specific test function
tools/devtool -y test -- integration_tests/functional/test_api.py::test_api_happy_start

# Run with substring filter
tools/devtool -y test -- -k boottime integration_tests/performance/

# Performance tests require --performance flag
tools/devtool -y test --performance -- integration_tests/performance/test_boottime.py

# Debug mode with ipdb
tools/devtool -y test_debug -- integration_tests/functional/test_api.py --pdb

# Run in parallel (functional tests only)
tools/devtool -y test -- integration_tests/functional -n8 --dist worksteal
```

**Rust unit/integration tests**:
```bash
# Run Rust unit tests
cargo test

# Run Rust integration tests only
cargo test --test integration_tests --all
```

**Test markers** (pytest):
- `nonci` - skipped in PR CI, run in scheduled pipelines
- `no_block_pr` - optional PR CI, not required for merge

## Style & Verification

```bash
# Run all style checks (lint, clippy, markdown, etc.)
tools/devtool checkstyle

# Auto-format Rust code
tools/devtool fmt

# Check build for all architectures
tools/devtool checkbuild --all

# Check build for specific arch
tools/devtool checkbuild -m x86_64
```

**Recommended pre-commit hook**:
```bash
cat >> .git/hooks/pre-commit << EOF
./tools/devtool checkstyle || exit 1
./tools/devtool checkbuild --all || exit 1
EOF
```

## Prerequisites

- **KVM access**: `/dev/kvm` must exist and be RW-accessible
- **Docker**: Required for all devtool commands
- **Linux bare-metal host**: kernel >= 5.10 (not virtualized)
- **Architecture**: x86_64 or aarch64 only

Check prerequisites:
```bash
tools/devtool checkenv
```

## Architecture

**Rust workspace** (`Cargo.toml`):
- `src/firecracker` - main VMM binary, HTTP API server
- `src/vmm` - core VMM library (devices, memory, CPU emulation)
- `src/jailer` - production sandboxing process (excluded from default build)
- `src/seccompiler` - seccomp filter compiler
- `src/utils` - shared utilities
- `src/acpi-tables` - ACPI table generation

**Edition**: Rust 2024
**Toolchain**: 1.96.0 (see `rust-toolchain.toml`)
**Targets**: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`

**Thread model** (per process):
- API thread - HTTP server, control plane
- VMM thread - device emulation, MMDS
- vCPU threads - one per guest CPU core (KVM_RUN loop)

## Code Style Requirements

**Unsafe code** (heavily discouraged, strict requirements):
```rust
// SAFETY: [list all invariants upheld]
// JUSTIFICATION: [why unsafe is necessary, alternatives considered]
unsafe {
    // ...
}
```

- Must satisfy `clippy::undocumented_unsafe_blocks`
- Include quantifiable justification (e.g., benchmark results) for performance claims

**Clippy lints** (workspace-level, enforced):
- `undocumented_unsafe_blocks`, `ptr_as_ptr`, `cast_possible_truncation`
- `cast_possible_wrap`, `cast_sign_loss`, `exit`
- `tests_outside_test_module`, `assertions_on_result_states`

**Avoid**:
- `Option::unwrap`/`Result::unwrap` - prefer error propagation or documented `expect`
- `as any`, `@ts-ignore` (not applicable - Rust project)

## Integration Test Patterns

**Fixture hierarchy** (pytest):
- `uvm` - minimal microVM, caller drives spawn/config/start
- `uvm_configured` - spawned + basic_config done
- `uvm_booted` - ready for SSH
- `uvm_restored` - snapshot restored
- `uvm_any` - parametrized over booted + restored

**Pin guest kernel** for kernel-independent tests:
```python
from framework.artifacts import pin_guest_kernel, GUEST_KERNEL_DEFAULT

@pin_guest_kernel(GUEST_KERNEL_DEFAULT)
def test_foo(uvm_booted):
    ...
```

## Special Commands

```bash
# Interactive shell in dev container
tools/devtool shell

# Privileged shell (for jailer tests)
tools/devtool shell --privileged

# Interactive IPython sandbox with microVM
tools/devtool -y sandbox

# Generate Rust documentation
tools/devtool mkdocs

# Download CI artifacts from S3
tools/devtool download_ci_artifacts [s3_uri...]

# Install binaries to /usr/local/bin
tools/devtool install --release
```

## CI Pipeline

**PR CI** (`.buildkite/pipeline_pr.py`):
- Style check (always)
- Build tests (if Rust/TOML changed)
- Functional + security tests (parallel, -n 16)
- Performance tests (single-tenant, requires --performance)
- Kani verification (if Rust changed, slow)

**Doc-only changes**: Skip build/test steps

## Key Files

- `src/firecracker/swagger/firecracker.yaml` - OpenAPI spec
- `tests/framework/` - pytest framework, fixtures
- `tests/conftest.py` - test configuration
- `docs/getting-started.md` - setup guide
- `docs/design.md` - architecture overview
- `SPECIFICATION.md` - performance SLAs

## Common Mistakes

- Running `cargo build` directly instead of `tools/devtool build`
- Looking for binaries in `build/debug/` instead of `build/cargo_target/${toolchain}/debug/`
- Running tests without `-y` flag (prompts for confirmation)
- Running performance tests without `--performance` flag (fail due to missing host tuning)
- Using `unwrap()` without justification
- Missing SAFETY/JUSTIFICATION comments on unsafe blocks
- Forgetting DCO signoff on commits (`git commit -s`)