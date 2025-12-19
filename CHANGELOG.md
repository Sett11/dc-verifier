# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Progress bar support** using `indicatif = "0.17"` for long-running operations
  - Progress bars in `check` command for adapter processing and contract checking
  - Progress bars in `visualize` command for graph building and DOT generation
  - Spinner for report generation
- **JSON report format** support via `--format json` flag
  - `JsonReporter` fully integrated into CLI (`check.rs`)
  - JSON reports include version, timestamp (RFC3339), summary (total_chains, critical_issues, warnings), and full chain data
  - Accessible through `ReportFormat::Json` enum in `main.rs`
- **Configurable maximum recursion depth** via `max_recursion_depth` in config
  - Optional field in `Config` struct
  - Supported in `FastApiCallGraphBuilder::with_max_depth()` and `TypeScriptCallGraphBuilder::with_max_depth()`
  - Prevents infinite recursion in large projects
  - Error type `GraphError::MaxDepthExceeded` when limit is reached
- **Configuration validation** with detailed error messages
  - `Config::validate()` method checks all configuration fields
  - Validates adapter types, paths, output format, and required fields
  - Automatic validation on config load
  - Detailed error messages with adapter/field context
- **Custom error types** using `thiserror = "2.0"` for better error handling
  - New `error.rs` module with `ParseError`, `ConfigError`, `GraphError`, `ValidationError`, `DcError`
  - Automatic error conversion via `#[from]` attributes
  - Exported through `dc-core/src/lib.rs`
- **CONTRIBUTING.md** with contribution guidelines
  - Development process and code standards
  - Instructions for adding new adapters
  - Code review process
- **CHANGELOG.md** for tracking changes in Keep a Changelog format
- **NestJS adapter** (`dc-adapter-nestjs`) for TypeScript backend projects
  - Decorator-based route extraction (`@Controller`, `@Get`, `@Post`, `@Put`, `@Delete`, `@Patch`)
  - DTO class extraction with class-validator decorator support
  - Parameter extraction from `@Body()`, `@Query()`, `@Param()` decorators
  - Request/response type inference from method signatures
  - Integration with CLI (`check.rs`, `config.rs`, `init.rs`)
- **Frontend library support** for API call detection
  - TanStack Query (React Query) - `useQuery`, `useMutation` with type extraction
  - SWR - `useSWR`, `useSWRMutation` with type extraction
  - RTK Query (Redux Toolkit Query) - `*.use*Query()`, `*.use*Mutation()` patterns
  - tRPC - `.useQuery()`, `.useMutation()` chain patterns
  - Apollo Client - `useQuery`, `useMutation` with GraphQL queries
  - Next.js Server Actions - `actions.*()` function calls
- **Enhanced TypeScript parser** with decorator extraction
  - Class decorator extraction (`@Controller`, etc.)
  - Method decorator extraction (`@Get`, `@Post`, etc.)
  - Parameter decorator extraction (`@Body`, `@Query`, `@Param`)
  - Support for decorator arguments and keyword arguments
- **DTO class extraction** for NestJS projects
  - Detection of class-validator decorators (`@IsString`, `@IsEmail`, `@IsOptional`, etc.)
  - Field-level validation rule extraction
  - Schema reference creation for DTO classes
- **Severity levels** for contract mismatches
  - `SeverityLevel` enum: `Low`, `Medium`, `High`, `Critical`
  - Automatic severity assignment based on mismatch type
  - Enhanced report recommendations with severity levels
- **Chain type categorization**
  - `ChainType` enum: `Full`, `FrontendInternal`, `BackendInternal`
  - Automatic chain type detection based on node types
  - Statistics by chain type in reports

### Changed
- **All code comments** translated to English (main public functions and doc comments)
- **Improved error messages** with context using `anyhow::with_context()`
- **JsonReporter** now fully integrated into CLI (was previously marked as dead code)
- **TypeScript call graph builder** now supports multiple frontend libraries
- **Report format** enhanced with severity levels and chain type statistics
- **Configuration** now supports `nestjs` adapter type
- **README.md** updated with new features (progress bars, JSON reports, max_recursion_depth, thiserror, NestJS adapter, frontend libraries)
- **AUDIT_REPORT.md** updated with latest implementation details and test statistics

### Fixed
- Removed outdated TODO comments
- Fixed temporary value lifetime issues in progress bar messages
- Synchronized documentation across README, AUDIT_REPORT, and CHANGELOG

## [0.1.0] - 2025-11-26

### Added
- Initial release
- Python/FastAPI support
- TypeScript support
- Call graph building
- Data chain extraction
- Contract checking
- Markdown report generation
- DOT graph visualization
- Configuration file support
- Cache support for graph serialization
- Location converter for accurate line/column tracking
- TypeScript function and class extraction
- TypeScript interface and type alias parsing
- Zod schema extraction and synchronization with TypeScript types
- Integration tests for TypeScript
- Unit tests for core functionality

### Features
- Builds call graphs from Python and TypeScript code
- Extracts data chains from call graphs
- Validates contracts between chain links
- Generates reports in Markdown format
- Visualizes graphs in DOT format
- Supports Pydantic, Zod, TypeScript, OpenAPI, and JSON Schema

