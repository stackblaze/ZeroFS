# GitHub Actions Workflows

This directory contains GitHub Actions workflows for ZeroFS CI/CD.

## Active Workflows (Run on Push/PR)

### `rust.yml` - Rust CI
- **Triggers**: Push to main, Pull requests
- **Purpose**: Code quality checks
- **Jobs**:
  - `fmt`: Check code formatting with rustfmt
  - `clippy`: Run clippy linter
  - `build`: Build and run tests (including crash tests)

### `docker.yml` - Docker Build and Push
- **Triggers**: Tags (v*), Pull requests, Manual
- **Purpose**: Build multi-arch Docker images
- **Targets**: linux/amd64, linux/arm64, linux/arm/v7, linux/386
- **Registries**: 
  - Docker Hub: `docker.io/stackblaze/zerofs`
  - GHCR: `ghcr.io/stackblaze/zerofs`

### `release.yml` - GitHub Releases
- **Triggers**: Tags (v*)
- **Purpose**: Create GitHub releases with downloadable binaries
- **Platforms**: Linux (x86_64, ARM64, ARMv7), macOS (Intel, Apple Silicon), FreeBSD

## Manual Workflows (workflow_dispatch only)

These workflows are **disabled for automatic runs** to save CI resources. 
Run them manually from the GitHub Actions UI when needed.

### Testing Workflows
- `bench.yml` - Benchmark testing
- `stress-ng.yml` - Stress testing
- `test-action-minio.yml` - MinIO integration tests

### Filesystem Compliance Tests
- `pjdfstest.yml` - POSIX compliance tests (NFS)
- `pjdfstest-9p.yml` - POSIX compliance tests (9P)
- `xfstests-nfs.yml` - Extended filesystem tests (NFS)
- `xfstests-9p.yml` - Extended filesystem tests (9P)
- `zfs-test.yml` - ZFS on ZeroFS tests

### Performance Tests
- `kernel-compile-nfs.yml` - Linux kernel compilation test (NFS)
- `kernel-compile-9p.yml` - Linux kernel compilation test (9P)

### Build Workflows
- `cross-compile.yml` - Cross-platform compilation testing
- `release-pgo.yml` - Profile-Guided Optimization builds (very slow)

## Running Manual Workflows

To run a manual workflow:
1. Go to: https://github.com/stackblaze/ZeroFS/actions
2. Select the workflow from the left sidebar
3. Click "Run workflow" button
4. Select branch and click "Run workflow"

## CI Resource Management

The active workflows (rust.yml, docker.yml, release.yml) are optimized for:
- Fast feedback on code changes
- Automated releases on tags
- Multi-platform Docker images

Heavy testing workflows are manual-only to:
- Reduce CI costs
- Avoid unnecessary long-running jobs
- Run comprehensive tests only when needed

