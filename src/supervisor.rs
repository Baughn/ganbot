use std::collections::HashMap;

/// Root of the supervision tree.
/// This module contains the main logic for managing the application's lifecycle,
/// including starting and stopping modules, handling configuration changes,
/// and managing global state.
use kameo::{Actor, actor::ActorRef};

use crate::config::{Config, global::IrcConfig};

pub struct Supervisor {
    config: Config,
    // Supervised services:
    irc: HashMap<IrcConfig, ActorRef<crate::network::irc::IrcActor>>,
}

impl Actor for Supervisor {
    type Args = Config;
    type Error = anyhow::Error;

    #[tracing::instrument(skip(actor_ref))]
    async fn on_start(config: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        tracing::info!(
            "Starting Supervisor with {} IRC server(s)",
            config.irc.len()
        );

        let mut irc_actors = HashMap::new();
        for irc_config in &config.irc {
            let irc_actor_ref =
                crate::network::irc::IrcActor::spawn_link(&actor_ref, irc_config.clone()).await;
            irc_actors.insert(irc_config.clone(), irc_actor_ref);
        }

        Ok(Supervisor {
            config,
            irc: irc_actors,
        })
    }

    async fn on_link_died(
        &mut self,
        actor_ref: kameo::prelude::WeakActorRef<Self>,
        id: kameo::prelude::ActorID,
        reason: kameo::prelude::ActorStopReason,
    ) -> impl Future<
        Output = Result<std::ops::ControlFlow<kameo::prelude::ActorStopReason>, Self::Error>,
    > + Send {
        tracing::warn!("Linked actor {:?} died: {:?}", id, reason);
        // Handle actor death, possibly restart or clean up
    }
}
