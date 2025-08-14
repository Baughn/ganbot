use kameo::actor::ActorRef;

pub type Broker = ActorRef<kameo_actors::broker::Broker<Action>>;

/// Core list of actions that can be broker'd.
#[derive(Debug, Clone)]
pub enum Action {
    Ping,
    Ask(String),
}
