#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Visibility {
    pub visible: bool,
}

impl Visibility {
    pub fn visible() -> Self {
        Self { visible: true }
    }

    pub fn hidden() -> Self {
        Self { visible: false }
    }
}

#[cfg(test)]
mod tests {
    use super::Visibility;

    #[test]
    fn visibility_helpers() {
        assert!(Visibility::visible().visible);
        assert!(!Visibility::hidden().visible);
    }
}
