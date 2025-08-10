use std::collections::HashMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use kameo::prelude::{ActorID, ActorStopReason, WeakActorRef};
/// Root of the supervision tree.
/// This module contains the main logic for managing the application's lifecycle,
/// including starting and stopping modules, handling configuration changes,
/// and managing global state.
use kameo::{Actor, actor::ActorRef};

use crate::config::{Config, global::IrcConfig};

/// Tracks restart attempts for a specific actor
#[derive(Debug, Clone)]
struct RestartInfo {
    /// Number of consecutive failures
    failure_count: u32,
    /// Time of the last failure
    last_failure: Instant,
    /// Time of the first failure in this sequence
    first_failure: Instant,
    /// Configuration to use when restarting
    config: IrcConfig,
}

pub struct Supervisor {
    config: Config,
    // Supervised services:
    irc: HashMap<IrcConfig, ActorRef<crate::network::irc::IrcActor>>,
    // Track restart attempts per actor
    restart_tracker: HashMap<ActorID, RestartInfo>,
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
            restart_tracker: HashMap::new(),
        })
    }

    async fn on_link_died(
        &mut self,
        actor_ref: WeakActorRef<Self>,
        id: ActorID,
        reason: ActorStopReason,
    ) -> Result<ControlFlow<ActorStopReason>, Self::Error> {
        tracing::warn!("Linked actor {:?} died: {:?}", id, reason);

        // Find which IRC config this actor belonged to
        let mut matching_config = None;
        for (config, actor_ref) in &self.irc {
            if actor_ref.id() == id {
                matching_config = Some(config.clone());
                break;
            }
        }

        if let Some(config) = matching_config {
            let now = Instant::now();

            // Check if we have restart info for this actor
            let restart_info = if let Some(mut info) = self.restart_tracker.remove(&id) {
                // Check if it's been stable for > 5 minutes, reset if so
                if now.duration_since(info.last_failure) > Duration::from_secs(300) {
                    tracing::info!(
                        "Actor {:?} was stable for >5 minutes, resetting failure count",
                        id
                    );
                    RestartInfo {
                        failure_count: 1,
                        last_failure: now,
                        first_failure: now,
                        config: config.clone(),
                    }
                } else {
                    // Still within failure window, increment count
                    info.failure_count += 1;
                    info.last_failure = now;
                    info
                }
            } else {
                // First failure for this actor
                RestartInfo {
                    failure_count: 1,
                    last_failure: now,
                    first_failure: now,
                    config: config.clone(),
                }
            };

            // Check if we've been failing for more than 5 minutes
            if now.duration_since(restart_info.first_failure) > Duration::from_secs(300) {
                tracing::error!(
                    "Actor for IRC server {} has been failing for >5 minutes, giving up",
                    restart_info.config.server
                );
                return Ok(ControlFlow::Break(ActorStopReason::Killed));
            }

            // Calculate exponential backoff (1s, 2s, 4s, 8s, ..., capped at 5 minutes)
            let backoff_secs = 2_u64.pow(restart_info.failure_count.min(8) - 1).min(300);
            let backoff = Duration::from_secs(backoff_secs);

            tracing::info!(
                "Will restart IRC actor for {} after {}s (attempt #{})",
                restart_info.config.server,
                backoff_secs,
                restart_info.failure_count
            );

            // Sleep for backoff duration
            tokio::time::sleep(backoff).await;

            // Attempt to restart the actor
            if let Some(strong_ref) = actor_ref.upgrade() {
                tracing::info!("Restarting IRC actor for {}", restart_info.config.server);
                let new_actor_ref = crate::network::irc::IrcActor::spawn_link(
                    &strong_ref,
                    restart_info.config.clone(),
                )
                .await;

                // Update our tracking with the new actor
                let new_id = new_actor_ref.id();
                self.irc.insert(restart_info.config.clone(), new_actor_ref);
                self.restart_tracker.insert(new_id, restart_info);
            } else {
                tracing::error!("Failed to upgrade weak reference to supervisor");
                return Ok(ControlFlow::Break(ActorStopReason::Killed));
            }
        } else {
            tracing::warn!("Unknown actor {:?} died, not restarting", id);
        }

        Ok(ControlFlow::Continue(()))
    }
}
