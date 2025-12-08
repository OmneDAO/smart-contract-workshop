//! Parser module for the pysub compiler.
//!
//! This module is responsible for translating indentation-sensitive pysub
//! source files into a brace-delimited form that can be parsed with `pest`,
//! and exposes a `parse_program` helper that validates syntax and preserves
//! the normalized source for downstream stages (AST construction, semantic
//! checks, etc.).

use std::fmt;

use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct PysubParser;

/// Result of parsing a pysub source file. The normalized source (with
/// indentation rewritten into explicit braces) is stored so that downstream
/// stages can re-parse or inspect it without re-running the indentation
/// normalizer.
#[derive(Debug, Clone)]
pub struct ParsedProgram {
    normalized: String,
}

impl ParsedProgram {
    /// Returns the normalized (brace-delimited) source.
    pub fn normalized_source(&self) -> &str {
        &self.normalized
    }

    /// Re-parses the stored normalized source and returns a fresh iterator of
    /// parse pairs. This avoids holding references that depend on self-referential
    /// lifetimes while still providing access to the parse tree when needed.
    pub fn pairs(&self) -> Result<pest::iterators::Pairs<'_, Rule>, ParseError> {
        PysubParser::parse(Rule::program, &self.normalized).map_err(ParseError::from)
    }
}

/// Parse errors exposed by the parser.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error(transparent)]
    Indentation(#[from] IndentationError),

    #[error("syntax error: {0}")]
    Pest(Box<pest::error::Error<Rule>>),
}

impl From<pest::error::Error<Rule>> for ParseError {
    fn from(error: pest::error::Error<Rule>) -> Self {
        ParseError::Pest(Box::new(error))
    }
}

/// Errors produced while normalizing indentation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IndentationError {
    #[error("line {line}: tabs are not permitted in indentation")]
    TabsNotAllowed { line: usize },

    #[error("line {line}: indentation must be a multiple of 4 spaces")]
    InvalidWidth { line: usize },

    #[error(
		"line {line}: indentation increased by more than one level (expected {expected}, found {found})"
	)]
    UnexpectedIndent {
        line: usize,
        expected: usize,
        found: usize,
    },

    #[error("line {line}: indentation level does not match any previous level")]
    MismatchedDedent { line: usize, indent: usize },

    #[error("line {line}: block statement requires an indented body")]
    ExpectedIndent { line: usize },
}

/// Parses the provided source code, returning a [`ParsedProgram`] on success.
pub fn parse_program(source: &str) -> Result<ParsedProgram, ParseError> {
    let normalized = normalize_indentation(source)?;
    // Validate that the normalized source satisfies the grammar. The parse
    // pairs are discarded here but can be regenerated on demand via
    // `ParsedProgram::pairs`.
    PysubParser::parse(Rule::program, &normalized)?;
    Ok(ParsedProgram { normalized })
}

/// Normalizes indentation-based blocks into explicit brace-delimited blocks so
/// that the grammar can remain context-free.
pub fn normalize_indentation(source: &str) -> Result<String, IndentationError> {
    const INDENT_WIDTH: usize = 4;

    let mut result = String::new();
    let mut indent_stack = vec![0usize];
    let mut expect_indent_after_block = false;

    for (line_index, raw_line) in source.lines().enumerate() {
        let trimmed_end = raw_line.trim_end();

        if trimmed_end.is_empty() {
            result.push('\n');
            continue;
        }

        if trimmed_end.chars().any(|c| c == '\t') {
            return Err(IndentationError::TabsNotAllowed {
                line: line_index + 1,
            });
        }

        let leading_spaces = trimmed_end.chars().take_while(|c| *c == ' ').count();
        let content = &trimmed_end[leading_spaces..];

        if content.starts_with('#') {
            // Comments do not affect indentation expectations but should
            // preserve pending indent requirements.
            result.push_str(content);
            result.push('\n');
            continue;
        }

        if leading_spaces % INDENT_WIDTH != 0 {
            return Err(IndentationError::InvalidWidth {
                line: line_index + 1,
            });
        }

        let indent_level = leading_spaces / INDENT_WIDTH;
        let mut current_level = *indent_stack.last().expect("indent stack is never empty");

        if expect_indent_after_block && indent_level <= current_level {
            return Err(IndentationError::ExpectedIndent {
                line: line_index + 1,
            });
        }

        if indent_level > current_level {
            if indent_level != current_level + 1 {
                return Err(IndentationError::UnexpectedIndent {
                    line: line_index + 1,
                    expected: current_level + 1,
                    found: indent_level,
                });
            }
            result.push_str("{\n");
            indent_stack.push(indent_level);
        } else if indent_level < current_level {
            while let Some(&level) = indent_stack.last() {
                if level <= indent_level {
                    break;
                }
                indent_stack.pop();
                result.push_str("}\n");
            }

            current_level = *indent_stack
                .last()
                .expect("indent stack retains the base level");

            if indent_level != current_level {
                return Err(IndentationError::MismatchedDedent {
                    line: line_index + 1,
                    indent: indent_level,
                });
            }
        }

        if expect_indent_after_block {
            expect_indent_after_block = false;
        }

        result.push_str(content);
        result.push('\n');

        if content.ends_with(':') {
            expect_indent_after_block = true;
        }
    }

    if expect_indent_after_block {
        // Source ended without providing a body for the final block.
        return Err(IndentationError::ExpectedIndent {
            line: source.lines().count().max(1),
        });
    }

    while indent_stack.len() > 1 {
        indent_stack.pop();
        result.push_str("}\n");
    }

    Ok(result)
}

impl fmt::Display for ParsedProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.normalized.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_simple_block() {
        let source = "fn main():\n    return 1\n";
        let normalized = normalize_indentation(source).unwrap();
        assert!(normalized.contains("{\nreturn 1\n}\n"));
    }

    #[test]
    fn reject_tab_indentation() {
        let source = "fn main():\n\treturn 1\n";
        let err = normalize_indentation(source).unwrap_err();
        assert!(matches!(err, IndentationError::TabsNotAllowed { .. }));
    }
}
