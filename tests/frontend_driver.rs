use linear::frontend::{self, FrontendError, LowerError};

#[test]
fn compile_module_parses_and_lowers_source() {
    let lowered = frontend::compile_module("fn id(take x: U32) -> U32 { x }").unwrap();

    assert!(lowered.program.function_id("id").is_some());
}

#[test]
fn compile_module_reports_parse_diagnostics() {
    let err = frontend::compile_module("fn bad(x: U32) -> U32 { mut x }").unwrap_err();

    let FrontendError::Parse(diagnostics) = err else {
        panic!("expected parse error");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.span.is_some())
    );
}

#[test]
fn compile_module_reports_lower_errors() {
    let err = frontend::compile_module("fn bad(take x: Missing) -> Missing { x }").unwrap_err();

    assert!(matches!(
        err,
        FrontendError::Lower(LowerError::UnknownType(name)) if name == "Missing"
    ));
}
