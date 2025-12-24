# Contributing to dc-verifier

Thank you for your interest in contributing to dc-verifier (Data Chains Verifier)!

## Development Process

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. **Format code** (`cargo fmt`)
5. **Check for linting issues** (`cargo clippy --all-targets --all-features -- -D warnings`)
6. **Ensure code compiles without warnings** (`cargo check --all`)
7. **Ensure all tests pass** (`cargo test --all`)
8. **Update documentation** if adding new features:
   - Update `README.md` with new capabilities and examples
   - Update `CHANGELOG.md` in the `[Unreleased]` section
9. Commit your changes (`git commit -m 'Add amazing feature'`)
10. Push to the branch (`git push origin feature/amazing-feature`)
11. Open a Pull Request

## Code Standards

### Language
- **Comments**: All code comments and documentation should be in English
- **Variable names**: Use English names
- **Function names**: Use English names

### Code Formatting
- Use `cargo fmt` to format code before committing
- Use `cargo clippy` to check for common issues

### Documentation
- All public functions and structs must have doc comments (`///`)
- Include examples in doc comments where appropriate
- Use standard Rust doc comment format
- **When adding new features**, update:
  - `README.md` - add to "Features" section and examples
  - `CHANGELOG.md` - add entry in `[Unreleased]` section under appropriate category (Added/Changed/Fixed)

## Running Tests

```bash
# Run all tests
cargo test --all

# Run tests for a specific crate
cargo test -p dc-core

# Run tests with output
cargo test --all -- --nocapture
```

**Important:** All tests must pass before submitting a PR. If you're adding new functionality, include appropriate tests.

### Testing Reporters

When working with reporters (`JsonReporter`, `MarkdownReporter`), ensure:
- Both reporters are tested in CLI integration scenarios
- JSON output is valid and parseable
- Markdown output is properly formatted
- Progress bars work correctly in both `check` and `visualize` commands

## Adding a New Adapter

To add support for a new language/framework:

1. Create a new crate in `crates/dc-<adapter-name>/`
2. Implement the parser in `src/parsers/` (if needed)
3. Implement the call graph builder in `src/call_graph.rs`
   - Support `with_max_depth()` method for recursion depth limiting
   - Use `GraphError::MaxDepthExceeded` when depth limit is reached
4. Add the adapter to `dc-cli/src/commands/check.rs`:
   - Handle `max_recursion_depth` from config
   - Add progress bar support for adapter processing
   - Integrate with `DataFlowTracker` and `ChainBuilder`
   - Support OpenAPI integration if applicable:
     - Parse `openapi_path` from config (global or adapter-specific)
     - Use `OpenAPIParser::parse_file()` to load OpenAPI schema
     - Create `OpenAPILinker` instance and pass to call graph builder
     - Use `OpenAPILinker` to enhance routes and link schemas
5. Add the adapter to `dc-cli/src/commands/visualize.rs`:
   - Support graph building with progress bars
   - Generate DOT files for visualization
6. Add tests in `tests/`:
   - Unit tests for parser and call graph builder
   - Integration tests for end-to-end scenarios
   - Edge case tests for complex scenarios
7. Update documentation:
   - Add adapter description to `README.md`
   - Add configuration example to `README.md`
   - Add entry to `CHANGELOG.md`
8. Update `dc-cli/src/config.rs`:
   - Add adapter type validation in `Config::validate()`
   - Ensure proper error messages for missing required fields
   - Support `openapi_path` in adapter configuration (optional field)

**OpenAPI Integration:** If your adapter can benefit from OpenAPI schema integration:
- Use `OpenAPILinker` from `dc-core::openapi` to link schemas with code artifacts
- Implement `with_openapi_schema()` method in your call graph builder
- Use `OpenAPILinker::match_route_to_endpoint()` to match discovered routes with OpenAPI endpoints
- Use `OpenAPILinker::link_pydantic_to_openapi()` or `link_typescript_to_openapi()` to link schemas
- Create virtual routes for OpenAPI endpoints not found in code
- See `dc-adapter-fastapi` and `dc-adapter-typescript` as reference implementations

**Note for TypeScript adapters:** If your adapter uses decorators (like NestJS), you may need to:
- Extend `TypeScriptParser` to extract decorators from SWC AST
- Implement decorator processing logic in your call graph builder
- Support DTO class extraction if applicable
- See `dc-adapter-nestjs` as a reference implementation

## Code Review Process

1. All PRs require at least one approval
2. Code must pass all CI checks:
   - `cargo fmt` - code formatting
   - `cargo clippy --all` - linting
   - `cargo check --all` - compilation without warnings
   - `cargo test --all` - all tests passing
3. Maintainers will review for:
   - Code quality and adherence to standards
   - Test coverage (unit and integration tests)
   - Documentation completeness (README, CHANGELOG)
   - Performance considerations
   - Proper use of progress bars for long operations
   - Configuration validation and error handling
   - Proper error types using `thiserror` where applicable

## Questions?

If you have questions, please open an issue or contact the maintainers.

