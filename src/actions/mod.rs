use kameo::actor::ActorRef;

pub type Bus = ActorRef<kameo_actors::message_bus::MessageBus>;

/// Core list of actions that can be broker'd.
#[derive(Debug, Clone)]
pub enum Action {
    Ping,
    Ask(String),
}
