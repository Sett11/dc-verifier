use crate::decorators::NestJSDecoratorProcessor;
use crate::dto::DTOExtractor;
use crate::extractor::ParameterExtractor;
use anyhow::Result;
use dc_core::call_graph::CallGraph;
use dc_core::parsers::TypeScriptParser;
use dc_typescript::TypeScriptCallGraphBuilder;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Builder for NestJS call graph
pub struct NestJSCallGraphBuilder {
    typescript_builder: TypeScriptCallGraphBuilder,
    src_paths: Vec<PathBuf>,
    verbose: bool,
}

impl NestJSCallGraphBuilder {
    /// Creates a new NestJS call graph builder
    pub fn new(src_paths: Vec<PathBuf>) -> Self {
        Self {
            typescript_builder: TypeScriptCallGraphBuilder::new(src_paths.clone()),
            src_paths,
            verbose: false,
        }
    }

    /// Sets verbose mode
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Sets max recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        if let Some(depth) = max_depth {
            self.typescript_builder = self.typescript_builder.with_max_depth(Some(depth));
        }
        self
    }

    /// Builds the call graph
    pub fn build_graph(self) -> Result<CallGraph> {
        // 1. Build base graph through TypeScriptCallGraphBuilder
        let mut graph = self.typescript_builder.build_graph()?;

        if self.verbose {
            debug!("NestJS adapter: base graph built, processing decorators...");
        }

        // 2. Find all TypeScript files
        let mut files = Vec::new();
        for src_path in &self.src_paths {
            Self::find_ts_files(src_path, &mut files)?;
        }

        if self.verbose {
            debug!(
                file_count = files.len(),
                "NestJS adapter: found TypeScript files"
            );
        }

        // 3. Extract DTO classes from all files
        let mut dto_extractor = DTOExtractor::new();
        for file in &files {
            if let Err(err) = dto_extractor.extract_dto_classes(file) {
                if self.verbose {
                    debug!(
                        file_path = ?file,
                        error = %err,
                        "Error extracting DTOs"
                    );
                }
                // Continue processing other files
            }
        }

        // 4. Process decorators for each file
        let parser = TypeScriptParser::new();
        let parameter_extractor = ParameterExtractor::new().with_dto_extractor(dto_extractor);
        let mut decorator_processor = NestJSDecoratorProcessor::new(graph)
            .with_parameter_extractor(parameter_extractor);

        for file in files {
            if let Err(err) =
                Self::process_file_decorators(&parser, &mut decorator_processor, &file)
            {
                if self.verbose {
                    debug!(
                        file_path = ?file,
                        error = %err,
                        "Error processing decorators"
                    );
                }
                // Continue processing other files
            }
        }

        // 5. Return updated graph
        graph = decorator_processor.into_graph();

        if self.verbose {
            debug!("NestJS adapter: decorator processing complete");
        }

        Ok(graph)
    }

    /// Processes decorators in a single file
    fn process_file_decorators(
        parser: &TypeScriptParser,
        processor: &mut NestJSDecoratorProcessor,
        file: &Path,
    ) -> Result<()> {
        let (module, source, converter) = parser.parse_file(file)?;
        let file_path_str = file.to_string_lossy().to_string();
        let decorators = parser.extract_decorators(&module, &file_path_str, &converter, &source);
        processor.process_decorators(decorators)?;
        Ok(())
    }

    /// Recursively finds all TypeScript files in a directory or adds a single file
    fn find_ts_files(dir: &PathBuf, files: &mut Vec<PathBuf>) -> Result<()> {
        if dir.is_file() {
            if let Some(ext) = dir.extension() {
                if ext == "ts" || ext == "tsx" {
                    files.push(dir.clone());
                }
            }
            return Ok(());
        }

        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                Self::find_ts_files(&path, files)?;
            }
        }

        Ok(())
    }
}
