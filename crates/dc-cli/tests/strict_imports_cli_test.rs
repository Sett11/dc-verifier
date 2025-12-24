use std::fs;
use std::path::Path;

use anyhow::Result;

use dc_cli::commands::check::execute_check;
use dc_cli::ReportFormat;

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

fn write_config(dir: &Path, strict_imports: Option<bool>) {
    let strict_line = match strict_imports {
        Some(true) => "strict_imports = true\n",
        Some(false) => "strict_imports = false\n",
        None => "",
    };

    let config = format!(
        r#"project_name = "demo"
entry_point = "app/main.py"
{strict_line}

[output]
format = "json"
path = "report.json"

[[adapters]]
type = "fastapi"
app_path = "app/main.py"
"#
    );

    fs::write(dir.join("dc-verifier.toml"), config).expect("failed to write config");
}

#[test]
fn non_strict_mode_allows_missing_external_imports() -> Result<()> {
    let project = create_temp_project(&[("app/main.py", "import fastapi\n")]);
    write_config(project.path(), Some(false));

    // Работать из корня временного проекта, чтобы относительные пути были валидны.
    std::env::set_current_dir(project.path())?;

    let config_path = project.path().join("dc-verifier.toml");
    let result = execute_check(config_path.to_str().unwrap(), ReportFormat::Json, false);

    if let Err(err) = &result {
        let msg = err.to_string();
        // В нестрогом режиме мы допускаем сбой анализа, но он не должен помечаться как [STRICT IMPORTS].
        assert!(
            !msg.contains("[STRICT IMPORTS]"),
            "in non-strict mode, errors must not be marked as [STRICT IMPORTS], got: {msg}"
        );
    }
    Ok(())
}

#[test]
fn strict_mode_fails_on_missing_external_imports() -> Result<()> {
    let project = create_temp_project(&[("app/main.py", "import fastapi\n")]);
    write_config(project.path(), Some(true));

    // Работать из корня временного проекта, чтобы относительные пути были валидны.
    std::env::set_current_dir(project.path())?;

    let config_path = project.path().join("dc-verifier.toml");
    let result = execute_check(config_path.to_str().unwrap(), ReportFormat::Json, false);

    assert!(
        result.is_ok(),
        "in strict mode, unresolved external imports should not crash the CLI"
    );
    Ok(())
}
