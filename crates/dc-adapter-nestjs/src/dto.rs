use anyhow::Result;
use dc_core::models::{Location, SchemaReference, SchemaType};
use dc_core::parsers::{FunctionOrClass, TypeScriptDecorator, TypeScriptParser};
use std::collections::HashMap;
use std::path::Path;

/// Extractor for DTO classes with class-validator decorators
pub struct DTOExtractor {
    parser: TypeScriptParser,
    dto_classes: HashMap<String, SchemaReference>,
}

impl Default for DTOExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl DTOExtractor {
    /// Creates a new DTO extractor
    pub fn new() -> Self {
        Self {
            parser: TypeScriptParser::new(),
            dto_classes: HashMap::new(),
        }
    }

    /// Extracts DTO classes from a file
    pub fn extract_dto_classes(&mut self, file: &Path) -> Result<Vec<SchemaReference>> {
        // 1. Parse file
        let (module, source, converter) = self.parser.parse_file(file)?;
        let file_path_str = file.to_string_lossy().to_string();

        // 2. Extract decorators to find DTO classes
        let decorators =
            self.parser
                .extract_decorators(&module, &file_path_str, &converter, &source);

        // 3. Extract functions and classes
        let functions_and_classes =
            self.parser
                .extract_functions_and_classes(&module, &file_path_str, &converter);

        let mut dto_schemas = Vec::new();

        // 4. For each class, check if it has class-validator decorators
        for item in functions_and_classes {
            if let FunctionOrClass::Class { name, line, .. } = item {
                // Check if class has class-validator decorators on properties
                // This is a simplified check - in full implementation we'd parse class properties
                if self.has_class_validator_decorators(&decorators, &name) {
                    // Extract DTO fields (simplified - would need to parse class properties from AST)
                    let fields = self.extract_dto_fields_simple(&name, &decorators);

                    let schema = SchemaReference {
                        name: name.clone(),
                        schema_type: SchemaType::TypeScript,
                        location: Location {
                            file: file_path_str.clone(),
                            line,
                            column: None,
                        },
                        metadata: {
                            let mut meta = HashMap::new();
                            meta.insert("fields".to_string(), serde_json::to_string(&fields)?);
                            meta.insert("dto_type".to_string(), "class-validator".to_string());
                            meta
                        },
                    };

                    self.dto_classes.insert(name, schema.clone());
                    dto_schemas.push(schema);
                }
            }
        }

        Ok(dto_schemas)
    }

    /// Checks if a class has class-validator decorators
    fn has_class_validator_decorators(
        &self,
        decorators: &[TypeScriptDecorator],
        class_name: &str,
    ) -> bool {
        // Check for class-validator decorators on class properties
        // Common validators: IsString, IsEmail, IsOptional, IsNumber, Min, Max, Length, etc.
        let validator_names = [
            "IsString",
            "IsEmail",
            "IsOptional",
            "IsNumber",
            "Min",
            "Max",
            "Length",
            "IsArray",
            "IsObject",
            "IsBoolean",
            "IsDate",
            "IsEnum",
            "IsNotEmpty",
            "ApiProperty", // @nestjs/swagger
        ];

        decorators.iter().any(|d| {
            matches!(&d.target, dc_core::parsers::DecoratorTarget::Parameter { class, .. } if class == class_name)
                && validator_names.iter().any(|&name| d.name == name)
        })
    }

    /// Extracts DTO fields (simplified version)
    /// Full implementation would parse class properties from SWC AST
    fn extract_dto_fields_simple(
        &self,
        _class_name: &str,
        decorators: &[TypeScriptDecorator],
    ) -> Vec<serde_json::Value> {
        // Simplified extraction - in full implementation would parse class properties
        // For now, return empty vector - will be enhanced when parsing class properties
        let mut fields = Vec::new();

        // Group decorators by parameter (property)
        let mut param_decorators: HashMap<String, Vec<&TypeScriptDecorator>> = HashMap::new();
        for decorator in decorators {
            if let dc_core::parsers::DecoratorTarget::Parameter {
                class, parameter, ..
            } = &decorator.target
            {
                if class == _class_name {
                    param_decorators
                        .entry(parameter.clone())
                        .or_default()
                        .push(decorator);
                }
            }
        }

        // Create field info for each parameter with decorators
        for (param_name, decorators) in param_decorators {
            let validation: Vec<String> = decorators.iter().map(|d| d.name.clone()).collect();
            let optional = decorators.iter().any(|d| d.name == "IsOptional");

            fields.push(serde_json::json!({
                "name": param_name,
                "validation": validation,
                "optional": optional,
            }));
        }

        fields
    }

    /// Gets a DTO schema by class name
    pub fn get_dto_schema(&self, class_name: &str) -> Option<&SchemaReference> {
        self.dto_classes.get(class_name)
    }

    /// Extracts validation rules from decorators
    #[allow(dead_code)] // Will be used in implementation
    fn extract_validation_rules(&self, _decorators: &[TypeScriptDecorator]) -> Vec<ValidationRule> {
        // Convert decorators to validation rules
        // TODO: Implement
        Vec::new()
    }
}

/// DTO field information
#[allow(dead_code)] // Will be used in implementation
pub struct DTOField {
    pub name: String,
    pub type_info: dc_core::models::TypeInfo,
    pub validation_rules: Vec<ValidationRule>,
    pub optional: bool,
}

/// Validation rule from class-validator decorator
#[allow(dead_code)] // Will be used in implementation
pub struct ValidationRule {
    pub decorator: String, // "IsString", "IsEmail", "Min", "Max", etc.
    pub arguments: Vec<String>,
}
