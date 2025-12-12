# dc-verifier (Data Chains Verifier)

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0-blue.svg)](https://github.com/Sett11/dc-verifier)

A tool for verifying data chain integrity between application layers (Frontend → Backend → Database) **WITHOUT RUNNING the application**.

**dc-verifier** (also known as DCV) is a static analyzer that implements the Data Chains concept in practice.

## Concept

dc-verifier analyzes data flow through a call graph:
1. Finds entry points (main.py, app.py for Python; .ts/.tsx files for TypeScript)
2. Builds call graph: tracks imports → functions → classes → methods
3. Extracts data schemas (Pydantic models, Zod schemas, TypeScript interfaces)
4. Traverses the graph: main → route → handler → crud → model
5. Tracks data flow through the graph
6. Checks contracts at each stitch
7. Generates a report on mismatches

## Features

### Language and Framework Support
- ✅ **Python/FastAPI** - Python code parsing, FastAPI routes extraction, Pydantic models
- ✅ **TypeScript** - TypeScript code parsing, extraction of imports, calls, functions, classes, methods, Zod schemas, interfaces and type aliases

### Code Analysis
- ✅ **Call graph building** - automatic graph construction for Python and TypeScript projects
- ✅ **Data flow tracking** - tracks parameters and return values through the graph
- ✅ **Contract checking** - verifies data schema compliance at chain stitches

### Reports and Visualization
- ✅ **Report formats** - generates reports in Markdown (default) or JSON format
- ✅ **Graph visualization** - generates DOT format for call graph visualization
- ✅ **Progress bars** - visual feedback for long-running operations

### Performance and Configuration
- ✅ **Caching** - saves and loads graphs to speed up repeated checks
- ✅ **Recursion depth limiting** - configurable `max_recursion_depth` for large projects
- ✅ **Flexible configuration** - supports multiple adapters and validation rules
- ✅ **Configuration validation** - detailed error messages for incorrect configuration
- ✅ **Typed errors** - uses `thiserror` for better error handling

## Installation

> **Note for developers:** If you cloned the repository for development, use the links below or use a local build.

### Method 1: Install via Cargo (recommended)

If you have Rust and Cargo installed:

```bash
cargo install dc-verifier
```

> **Note:** To install via `cargo install`, the project must be published on [crates.io](https://crates.io). Until the project is published, use other installation methods.

After installation, `dc-verifier` will be available in PATH.

### Method 2: Build from source

Clone the repository and build the project:

```bash
git clone https://github.com/Sett11/dc-verifier.git
cd dc-verifier
cargo build --release
```

The binary will be located at `target/release/dc-verifier` (or `target/release/dc-verifier.exe` on Windows).

### Method 3: Download pre-built release

Download a pre-built binary for your platform from [Releases](https://github.com/Sett11/dc-verifier/releases):

- **Linux**: `dc-verifier-x86_64-unknown-linux-gnu.tar.gz`
- **macOS**: `dc-verifier-x86_64-apple-darwin.tar.gz` or `dc-verifier-aarch64-apple-darwin.tar.gz`
- **Windows**: `dc-verifier-x86_64-pc-windows-msvc.zip`

Extract the archive and add the binary to PATH or use the full path to it.

### Requirements

- **Rust 1.70+** (only for building from source)
- **Python 3.8+** (for FastAPI project analysis)
- **Node.js** (not required, but may be useful for TypeScript projects)

## Usage

### Initialize Configuration

```bash
dc-verifier init
```

Creates a `dc-verifier.toml` file with a configuration example.

### Check Chains

```bash
# Markdown format (default)
dc-verifier check

# JSON format
dc-verifier check --format json
```

Checks data chains according to the configuration and generates a report in Markdown or JSON format. Progress bars are displayed during execution to track adapter processing and contract checking.

### Visualize Graphs

```bash
dc-verifier visualize
```

Generates DOT files for call graph visualization. Files can be opened in Graphviz or online tools.

## Project Structure

- `crates/dc-core/` - Core: graph building, data flow analysis, parsers, analyzers
- `crates/dc-adapter-fastapi/` - FastAPI adapter (Python)
- `crates/dc-typescript/` - TypeScript adapter
- `crates/dc-cli/` - CLI tool

## Configuration

Example configuration for a project with Python/FastAPI and TypeScript:

```toml
project_name = "my-project"

# Maximum recursion depth (optional, None = unlimited)
# Useful for large projects to avoid infinite recursion
# max_recursion_depth = 100

[output]
format = "markdown"  # or "json"
path = "dc-verifier-report.md"

[[adapters]]
type = "fastapi"
app_path = "app/main.py"

[[adapters]]
type = "typescript"
src_paths = ["frontend/src", "shared"]

[rules]
type_mismatch = "critical"      # Type mismatch checking
missing_field = "warning"        # Missing field checking
unnormalized_data = "warning"   # Data normalization checking
```

### Adapters

#### FastAPI Adapter

```toml
[[adapters]]
type = "fastapi"
app_path = "app/main.py"  # Path to FastAPI application file
```

#### TypeScript Adapter

```toml
[[adapters]]
type = "typescript"
src_paths = ["src", "lib"]  # Directories with TypeScript files
```

**Note:** The configuration uses the `type` field (not `adapter_type`), which is automatically mapped to `adapter_type` when loading the configuration.

### Validation Rules

Validation rules define the severity level for different types of mismatches:

```toml
[rules]
type_mismatch = "critical"     # Type mismatch checking (critical/warning/info)
missing_field = "warning"       # Missing field checking (critical/warning/info)
unnormalized_data = "warning"  # Data normalization checking (critical/warning/info)
```

These rules are used to determine severity in contracts and affect the final statistics in reports.

## Usage Examples

### Python/FastAPI Project

```bash
# 1. Create configuration
dc-verifier init

# 2. Configure dc-verifier.toml
# Specify path to FastAPI application

# 3. Run check
dc-verifier check

# 4. View report
cat dc-verifier-report.md

# Or generate JSON report
dc-verifier check --format json
cat dc-verifier-report.json
```

### TypeScript Project

```bash
# 1. Create configuration
dc-verifier init

# 2. Configure dc-verifier.toml
# Specify directories with TypeScript files

# 3. Run check
dc-verifier check

# 4. Visualize graph
dc-verifier visualize
# Open generated .dot file in Graphviz or online tools

# 5. Check chains with JSON report
dc-verifier check --format json
```

### Mixed Project (Python + TypeScript)

```toml
project_name = "fullstack-app"

# Recursion depth limit for large projects
max_recursion_depth = 150

[output]
format = "json"  # Use JSON format for CI/CD integration
path = "dc-verifier-report.json"

[[adapters]]
type = "fastapi"
app_path = "backend/app/main.py"

[[adapters]]
type = "typescript"
src_paths = ["frontend/src"]

[rules]
type_mismatch = "critical"
missing_field = "warning"
unnormalized_data = "info"
```

## What is Checked

dc-verifier checks the following aspects of data chains:

1. **Type compliance** - verifies that data types match at chain stitches
2. **Required fields** - verifies that all required fields are present
3. **Data normalization** - checks validation (email, URL, patterns)

## Report Formats

dc-verifier supports two report formats:

### Markdown (default)
- **Human-readable format** with emojis and formatting
- Includes statistics (total_chains, critical_issues, warnings, valid_chains)
- Detailed information about each chain with data paths and checked stitches
- Usage: `dc-verifier check` or `dc-verifier check --format markdown`

### JSON
- **Machine-readable format** for CI/CD integration and other tools
- Includes report version, timestamp (RFC3339), summary and full chain data
- Structured format for automated processing
- Usage: `dc-verifier check --format json`

Both formats contain the same information, but are presented in different formats for convenience.

## Requirements

- Rust 1.70+
- Python 3.8+ (for FastAPI adapter)
- Node.js (not required, but may be useful for TypeScript projects)

## Project Status

The project is ready for use. Current readiness: **~98-100%**.

### Implemented

- ✅ Python and TypeScript code parsing
- ✅ Call graph building for Python and TypeScript
- ✅ Function, class and method extraction from TypeScript AST
- ✅ TypeScript interface and type alias extraction
- ✅ Zod schema linking with TypeScript types
- ✅ TypeScript schema parsing to JsonSchema
- ✅ Data flow tracking
- ✅ Contract checking
- ✅ Graph visualization
- ✅ Graph caching
- ✅ CLI interface
- ✅ Configuration validation with detailed error messages (`Config::validate()`)
- ✅ Recursion depth limiting for large projects (`max_recursion_depth` in config)
- ✅ Progress bars for long-running operations (`indicatif::ProgressBar` in `check` and `visualize`)
- ✅ JSON report format (option `--format json`, `JsonReporter`)
- ✅ Improved error handling (typed errors via `thiserror`)
- ✅ Unit and integration tests (all tests pass)

### Planned

- ⚠️ Documentation improvements (usage examples)

## License

See LICENSE file (or Cargo.toml for license information).

## Contribution

The project is open for contributions! See details in [CONTRIBUTING.md](CONTRIBUTING.md).

**Important:** When adding new features or modifying existing ones, please update:
- `README.md` - feature descriptions and usage examples
- `AUDIT_REPORT.md` - detailed implementation information
- `CHANGELOG.md` - change history in Keep a Changelog format
