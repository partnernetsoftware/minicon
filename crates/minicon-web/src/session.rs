//! Engine-neutral commands for browser shells. Legacy `PageHost` remains usable.
//! A session owns page state; a native view is only one possible presentation.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageCommand {
    Navigate(String),
    Back,
    Forward,
    Reload,
    Inspect,
    Click(NodeReference),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeReference {
    pub target: String,
    pub revision: u64,
    pub node: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticNode {
    pub reference: NodeReference,
    pub role: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageView {
    pub state: PageState,
    /// `None` means this backend does not offer semantic snapshots.
    pub nodes: Option<Vec<SemanticNode>>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageError {
    pub code: String,
    pub message: String,
}

impl PageError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PageError {}

/// Implementations preserve their own target identity across navigation and
/// report unsupported operations explicitly. Commands do not imply that a
/// window exists, or that a backend can migrate a live page to another engine.
pub trait PageSession {
    fn dispatch(&mut self, command: PageCommand) -> Result<PageView, PageError>;
}
