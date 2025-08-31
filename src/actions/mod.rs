/// Core list of user actions.
#[derive(Debug, Clone)]
pub enum Action {
    Ping,
    Ask(String),
}
