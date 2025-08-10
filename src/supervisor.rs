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
pub(crate) struct RestartInfo {
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Time of the last failure
    pub last_failure: Instant,
    /// Time of the first failure in this sequence
    pub first_failure: Instant,
    /// Configuration to use when restarting
    pub config: IrcConfig,
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
            
            // Use the pure function to calculate restart strategy
            let existing_info = self.restart_tracker.remove(&id);
            let (restart_info, backoff, should_give_up) = 
                Self::calculate_restart_strategy(now, existing_info, config.clone());

            // Check if we should give up
            if should_give_up {
                tracing::error!(
                    "Actor for IRC server {} has been failing for >5 minutes, giving up",
                    restart_info.config.server
                );
                return Ok(ControlFlow::Break(ActorStopReason::Killed));
            }

            tracing::info!(
                "Will restart IRC actor for {} after {}s (attempt #{})",
                restart_info.config.server,
                backoff.as_secs(),
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

impl Supervisor {
    /// Pure function to calculate restart strategy based on failure history
    /// Returns (updated_restart_info, backoff_duration, should_give_up)
    pub(crate) fn calculate_restart_strategy(
        now: Instant,
        existing_info: Option<RestartInfo>,
        config: IrcConfig,
    ) -> (RestartInfo, Duration, bool) {
        let restart_info = if let Some(mut info) = existing_info {
            // Check if it's been stable for > 5 minutes, reset if so
            if now.duration_since(info.last_failure) > Duration::from_secs(300) {
                RestartInfo {
                    failure_count: 1,
                    last_failure: now,
                    first_failure: now,
                    config,
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
                config,
            }
        };

        // Check if we've been failing for more than 5 minutes
        let should_give_up = now.duration_since(restart_info.first_failure) > Duration::from_secs(300);

        // Calculate exponential backoff (1s, 2s, 4s, 8s, ..., capped at 5 minutes)
        let backoff_secs = (2_u64.pow(restart_info.failure_count.saturating_sub(1))).min(300);
        let backoff = Duration::from_secs(backoff_secs);

        (restart_info, backoff, should_give_up)
    }

    #[cfg(test)]
    pub(crate) fn get_restart_tracker(&self) -> &HashMap<ActorID, RestartInfo> {
        &self.restart_tracker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_first_failure() {
        let now = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        let (restart_info, backoff, should_give_up) = 
            Supervisor::calculate_restart_strategy(now, None, config.clone());

        assert_eq!(restart_info.failure_count, 1);
        assert_eq!(restart_info.last_failure, now);
        assert_eq!(restart_info.first_failure, now);
        assert_eq!(backoff, Duration::from_secs(1)); // First failure = 1s backoff
        assert!(!should_give_up);
    }

    #[test]
    fn test_exponential_backoff_progression() {
        let now = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        // Expected backoffs: 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, then capped at 300s
        let expected_backoffs = vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300];
        
        let mut current_info: Option<RestartInfo> = None;
        
        for (i, expected_secs) in expected_backoffs.iter().enumerate() {
            let (restart_info, backoff, _) = 
                Supervisor::calculate_restart_strategy(now, current_info, config.clone());
            
            assert_eq!(restart_info.failure_count, (i + 1) as u32);
            assert_eq!(backoff, Duration::from_secs(*expected_secs), 
                      "Failed at attempt {}: expected {}s, got {:?}", 
                      i + 1, expected_secs, backoff);
            
            current_info = Some(restart_info);
        }
    }

    #[test]
    fn test_stability_reset_after_5_minutes() {
        let base_time = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        // Create an existing failure from >5 minutes ago
        let old_info = RestartInfo {
            failure_count: 5,
            last_failure: base_time - Duration::from_secs(301), // 5 minutes + 1 second ago
            first_failure: base_time - Duration::from_secs(400),
            config: config.clone(),
        };

        let now = base_time;
        let (restart_info, backoff, should_give_up) = 
            Supervisor::calculate_restart_strategy(now, Some(old_info), config);

        // Should reset to first failure
        assert_eq!(restart_info.failure_count, 1);
        assert_eq!(restart_info.last_failure, now);
        assert_eq!(restart_info.first_failure, now);
        assert_eq!(backoff, Duration::from_secs(1));
        assert!(!should_give_up);
    }

    #[test]
    fn test_no_reset_within_5_minutes() {
        let base_time = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        // Create an existing failure from <5 minutes ago
        let old_info = RestartInfo {
            failure_count: 3,
            last_failure: base_time - Duration::from_secs(299), // Just under 5 minutes ago
            first_failure: base_time - Duration::from_secs(299),
            config: config.clone(),
        };

        let now = base_time;
        let (restart_info, backoff, should_give_up) = 
            Supervisor::calculate_restart_strategy(now, Some(old_info), config);

        // Should increment failure count, not reset
        assert_eq!(restart_info.failure_count, 4);
        assert_eq!(restart_info.last_failure, now);
        assert_eq!(restart_info.first_failure, base_time - Duration::from_secs(299));
        assert_eq!(backoff, Duration::from_secs(8)); // 4th failure = 8s backoff
        assert!(!should_give_up);
    }

    #[test]
    fn test_give_up_after_5_minutes_of_failures() {
        let base_time = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        // Create a failure that started >5 minutes ago but last failed recently
        let old_info = RestartInfo {
            failure_count: 10,
            last_failure: base_time - Duration::from_secs(10), // Recent failure
            first_failure: base_time - Duration::from_secs(301), // Started failing >5 minutes ago
            config: config.clone(),
        };

        let now = base_time;
        let (restart_info, _, should_give_up) = 
            Supervisor::calculate_restart_strategy(now, Some(old_info), config);

        // Should give up because first_failure was >5 minutes ago
        assert!(should_give_up);
        assert_eq!(restart_info.failure_count, 11);
    }

    #[test]
    fn test_dont_give_up_before_5_minutes() {
        let base_time = Instant::now();
        let config = IrcConfig {
            server: "test.server".to_string(),
            port: 6667,
            tls: false,
            nick: "testbot".to_string(),
            channels: vec![],
            nickserv_password: None,
        };

        // Create a failure that started <5 minutes ago
        let old_info = RestartInfo {
            failure_count: 20,
            last_failure: base_time - Duration::from_secs(10),
            first_failure: base_time - Duration::from_secs(299), // Just under 5 minutes
            config: config.clone(),
        };

        let now = base_time;
        let (_, _, should_give_up) = 
            Supervisor::calculate_restart_strategy(now, Some(old_info), config);

        // Should NOT give up yet
        assert!(!should_give_up);
    }
}
