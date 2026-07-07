use super::diagnostic::Diagnostic;
use super::lower::{LowerError, LoweredModule, lower_module_bodies};
use super::parse::parse_module_diagnostics;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendError {
    Parse(Vec<Diagnostic>),
    Lower(LowerError),
}

pub fn compile_module(src: &str) -> Result<LoweredModule, FrontendError> {
    let module = parse_module_diagnostics(src).map_err(FrontendError::Parse)?;
    lower_module_bodies(&module).map_err(FrontendError::Lower)
}
