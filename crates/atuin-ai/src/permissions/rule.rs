use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

static RULE_RE: OnceLock<Regex> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuleError {
    #[error("invalid rule format: {0}")]
    InvalidRule(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rule {
    pub tool: String,
    pub scope: Option<String>,
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.scope.as_ref() {
            Some(scope) => write!(f, "{}({})", self.tool, scope),
            None => write!(f, "{}", self.tool),
        }
    }
}

impl TryFrom<&str> for Rule {
    type Error = RuleError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        let re = RULE_RE.get_or_init(|| Regex::new(r"^(\w+)(?:\((.*)\))?$").unwrap());
        let caps = re
            .captures(value)
            .ok_or(RuleError::InvalidRule(value.to_string()))?;
        let tool = caps.get(1).unwrap().as_str().to_string();
        let scope = caps.get(2).map(|m| m.as_str().to_string());
        Ok(Rule { tool, scope })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_try_from() {
        assert_eq!(
            Rule::try_from("Read").unwrap(),
            Rule {
                tool: "Read".to_string(),
                scope: None
            }
        );
        assert_eq!(
            Rule::try_from("Read(*)").unwrap(),
            Rule {
                tool: "Read".to_string(),
                scope: Some("*".to_string())
            }
        );
        assert_eq!(
            Rule::try_from("Write(*.md)").unwrap(),
            Rule {
                tool: "Write".to_string(),
                scope: Some("*.md".to_string())
            }
        );
        assert_eq!(
            Rule::try_from("Shell(git commit *)").unwrap(),
            Rule {
                tool: "Shell".to_string(),
                scope: Some("git commit *".to_string())
            }
        );
        assert_eq!(
            Rule::try_from("Shell(echo ())").unwrap(),
            Rule {
                tool: "Shell".to_string(),
                scope: Some("echo ()".to_string())
            }
        );
        assert!(Rule::try_from("Shell(git commit *").is_err());
        assert!(Rule::try_from("Shell(git commit *)!").is_err());
    }
}
