#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionKind {
    Import,
    Require,
}

impl ResolutionKind {
    const fn condition(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Require => "require",
        }
    }
}

/// Options that control package and module resolution.
///
/// Browser condition handling is opt-in. The default condition set preserves the
/// historical resolver behavior: `wjsm`, `node`, the edge kind (`import` or
/// `require`), then `default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionOptions {
    browser: bool,
    custom_conditions: Vec<String>,
    import_conditions: Vec<String>,
    require_conditions: Vec<String>,
}

impl Default for ResolutionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolutionOptions {
    /// Creates resolver options with browser resolution disabled.
    pub fn new() -> Self {
        Self::from_parts(false, Vec::new())
    }

    /// Enables or disables the explicit `browser` condition.
    #[must_use]
    pub fn with_browser(mut self, browser: bool) -> Self {
        self.browser = browser;
        self.rebuild_conditions();
        self
    }

    /// Adds one custom package condition.
    #[must_use]
    pub fn with_condition<S>(mut self, condition: S) -> Self
    where
        S: Into<String>,
    {
        let condition = condition.into();
        if condition == "browser" {
            self.browser = true;
        }
        self.custom_conditions.push(condition);
        self.rebuild_conditions();
        self
    }

    /// Adds custom package conditions in caller-provided order.
    #[must_use]
    pub fn with_conditions<I, S>(mut self, conditions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for condition in conditions {
            let condition = condition.into();
            if condition == "browser" {
                self.browser = true;
            }
            self.custom_conditions.push(condition);
        }
        self.rebuild_conditions();
        self
    }

    /// Returns whether explicit browser resolution semantics are enabled.
    pub fn browser(&self) -> bool {
        self.browser
    }

    /// Returns import-edge package conditions.
    pub fn conditions(&self) -> &[String] {
        self.conditions_for_kind(ResolutionKind::Import)
    }

    pub(crate) fn conditions_for_kind(&self, kind: ResolutionKind) -> &[String] {
        match kind {
            ResolutionKind::Import => &self.import_conditions,
            ResolutionKind::Require => &self.require_conditions,
        }
    }

    fn from_parts(browser: bool, custom_conditions: Vec<String>) -> Self {
        let mut options = Self {
            browser,
            custom_conditions,
            import_conditions: Vec::new(),
            require_conditions: Vec::new(),
        };
        options.rebuild_conditions();
        options
    }

    fn rebuild_conditions(&mut self) {
        self.import_conditions = build_conditions(
            self.browser,
            &self.custom_conditions,
            ResolutionKind::Import,
        );
        self.require_conditions = build_conditions(
            self.browser,
            &self.custom_conditions,
            ResolutionKind::Require,
        );
    }
}

fn build_conditions(
    browser: bool,
    custom_conditions: &[String],
    kind: ResolutionKind,
) -> Vec<String> {
    let mut conditions = Vec::with_capacity(custom_conditions.len() + 5);
    push_unique_condition(&mut conditions, "wjsm");
    if browser {
        push_unique_condition(&mut conditions, "browser");
    }
    for condition in custom_conditions {
        if !is_reserved_custom_condition(condition) {
            push_unique_condition(&mut conditions, condition);
        }
    }
    push_unique_condition(&mut conditions, "node");
    push_unique_condition(&mut conditions, kind.condition());
    push_unique_condition(&mut conditions, "default");
    conditions
}

fn push_unique_condition(conditions: &mut Vec<String>, condition: &str) {
    if !conditions.iter().any(|existing| existing == condition) {
        conditions.push(condition.to_string());
    }
}

fn is_reserved_custom_condition(condition: &str) -> bool {
    matches!(
        condition,
        "wjsm" | "browser" | "node" | "import" | "require" | "default"
    )
}

#[cfg(test)]
mod tests {
    use super::ResolutionOptions;

    #[test]
    fn browser_condition_enables_browser_semantics() {
        let options = ResolutionOptions::new().with_condition("browser");

        assert!(options.browser());
        assert_eq!(
            options.conditions(),
            &["wjsm", "browser", "node", "import", "default"]
        );
    }

    #[test]
    fn browser_condition_in_list_enables_browser_semantics_once() {
        let options = ResolutionOptions::new().with_conditions(["browser", "custom"]);

        assert!(options.browser());
        assert_eq!(
            options.conditions(),
            &["wjsm", "browser", "custom", "node", "import", "default"]
        );
    }
}
