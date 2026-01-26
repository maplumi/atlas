use foundation::time::TimeSpan;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ComponentTimeSpan {
    pub span: TimeSpan,
}

impl ComponentTimeSpan {
    pub fn new(span: TimeSpan) -> Self {
        Self { span }
    }
}
