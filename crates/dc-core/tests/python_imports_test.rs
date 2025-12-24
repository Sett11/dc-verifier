use std::fs;
use std::path::Path;

use dc_core::parsers::python::PythonParser;

fn create_temp_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    for (path, content) in files {
        let full_path = tmp_dir.path().join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        fs::write(&full_path, content).expect("failed to write file");
    }
    tmp_dir
}

#[test]
fn resolve_import_safe_treats_non_external_as_local_missing() {
    let project = create_temp_project(&[("app/main.py", "import local_module\n")]);
    let parser = PythonParser::new();

    let result = parser
        .resolve_import_safe("local_module", Path::new("app/main.py"), project.path())
        .expect("resolve_import_safe should not fail for local modules");

    assert!(
        result.is_none(),
        "local module without package files should be treated as missing but non-fatal"
    );
}

#[test]
fn resolve_import_safe_fails_for_missing_external_dependency() {
    let project = create_temp_project(&[("requirements.txt", "fastapi==0.111.0\n")]);
    let parser = PythonParser::new();

    let err = parser
        .resolve_import_safe("fastapi", Path::new("app/main.py"), project.path())
        .expect_err("missing external dependency should return ImportError");

    match err {
        dc_core::models::ImportError::ExternalDependency { module, suggestion } => {
            assert_eq!(module, "fastapi");
            assert!(
                suggestion.contains("pip install fastapi"),
                "suggestion should contain installation hint"
            );
        }
        other => panic!("unexpected ImportError variant: {:?}", other),
    }
}

#[test]
fn resolve_import_cached_uses_cache_for_repeated_imports() {
    let project = create_temp_project(&[("requirements.txt", "fastapi==0.111.0\n")]);
    let mut parser = PythonParser::new();

    // First call fills the cache (and fails with ExternalDependency)
    let first_err = parser
        .resolve_import_cached("fastapi", project.path())
        .expect_err("first call should still propagate ImportError");

    match first_err {
        dc_core::models::ImportError::ExternalDependency { module, .. } => {
            assert_eq!(module, "fastapi");
        }
        other => panic!("unexpected ImportError variant: {:?}", other),
    }

    // Second call should hit the cache and return the same error semantics
    let second_err = parser
        .resolve_import_cached("fastapi", project.path())
        .expect_err("second call should also propagate ImportError");

    match second_err {
        dc_core::models::ImportError::ExternalDependency { module, .. } => {
            assert_eq!(module, "fastapi");
        }
        other => panic!("unexpected ImportError variant on second call: {:?}", other),
    }
}
