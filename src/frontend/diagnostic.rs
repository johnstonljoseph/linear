#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineColumnSpan {
    pub start: SourceLocation,
    pub end: SourceLocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Option<SourceSpan>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }

    pub fn spanned(message: impl Into<String>, span: SourceSpan) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
        }
    }
}

impl SourceSpan {
    pub fn line_columns(self, src: &str) -> LineColumnSpan {
        LineColumnSpan {
            start: line_column_at(src, self.start),
            end: line_column_at(src, self.end),
        }
    }
}

pub fn line_column_at(src: &str, byte_offset: usize) -> SourceLocation {
    let mut line = 1;
    let mut column = 1;

    for (index, ch) in src.char_indices() {
        if index >= byte_offset {
            break;
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    SourceLocation { line, column }
}
