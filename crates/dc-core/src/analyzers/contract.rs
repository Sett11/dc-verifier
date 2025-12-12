use crate::analyzers::ContractRule;
use crate::models::{Contract, Mismatch};

/// Contract checker - applies rules to contracts
pub struct ContractChecker {
    rules: Vec<Box<dyn ContractRule>>,
}

impl ContractChecker {
    /// Creates a new checker with default rules
    pub fn new() -> Self {
        let mut checker = Self { rules: Vec::new() };

        // Add default rules
        checker.add_rule(Box::new(crate::analyzers::TypeMismatchRule));
        checker.add_rule(Box::new(crate::analyzers::MissingFieldRule));
        checker.add_rule(Box::new(crate::analyzers::UnnormalizedDataRule));

        checker
    }

    /// Adds a checking rule
    pub fn add_rule(&mut self, rule: Box<dyn ContractRule>) {
        self.rules.push(rule);
    }

    /// Checks contract between two links
    pub fn check_contract(&self, contract: &Contract) -> Vec<Mismatch> {
        let mut all_mismatches = Vec::new();

        for rule in &self.rules {
            let mismatches = rule.check(contract);
            all_mismatches.extend(mismatches);
        }

        all_mismatches
    }

    /// Compares two schemas and finds mismatches
    pub fn compare_schemas(
        &self,
        from: &crate::models::SchemaReference,
        to: &crate::models::SchemaReference,
    ) -> Vec<Mismatch> {
        // Create temporary contract for checking
        let contract = Contract {
            from_link_id: String::new(),
            to_link_id: String::new(),
            from_schema: from.clone(),
            to_schema: to.clone(),
            mismatches: Vec::new(),
            severity: crate::models::Severity::Info,
        };

        // Use all rules for checking
        self.check_contract(&contract)
    }
}

impl Default for ContractChecker {
    fn default() -> Self {
        Self::new()
    }
}
