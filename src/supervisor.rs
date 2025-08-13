use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher as _};
/// Root of the supervision tree.
/// This module contains the main logic for managing the application's lifecycle,
/// including starting and stopping modules, handling configuration changes,
/// and managing global state.
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::future::join_all;
use kameo::error::Infallible;
use kameo::prelude::*;
use kameo::{Actor, actor::ActorRef};
use tokio::time::sleep;
use tracing::{error, info, instrument};

use crate::config::{Config, global::IrcConfig};
use crate::network::irc::IrcActor;

type ConfigHash = u64;

pub struct Supervisor {
    /// Currently active configuration.
    config: Config,
    /// Actors implied by said configuration.
    actors: HashMap<ConfigHash, Supervised>,
}

/// Tracks restart attempts for a specific actor
#[derive(Debug, Clone)]
struct RestartInfo {
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Time of the last failure
    pub last_failure: Instant,
    /// Time of the first failure in this sequence
    pub first_failure: Instant,
    /// Amount of time we sleep for the most recent failure.
    pub sleep: Duration,
}

/// The necessary information to restart an actor.
#[derive(Hash)]
enum SomeActor {
    Irc((IrcConfig, ActorRef<IrcActor>)),
}

struct Supervised {
    restart_info: RestartInfo,
    actor: SomeActor,
}

struct ReloadConfig;
struct ApplyConfig;

impl Actor for Supervisor {
    type Args = Config;
    type Error = Infallible;

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        actor_ref.register("supervisor").unwrap();
        // Queue the config application.
        actor_ref.tell(ApplyConfig).try_send().unwrap();

        Ok(Supervisor {
            config: args,
            actors: HashMap::new(),
        })
    }

    #[instrument(skip(self, actor_ref, reason))]
    async fn on_link_died(
        &mut self,
        actor_ref: WeakActorRef<Self>,
        id: ActorID,
        reason: ActorStopReason,
    ) -> std::result::Result<ControlFlow<ActorStopReason>, Self::Error> {
        // Check for dead actors & restart them.
        let self_reference = actor_ref.upgrade().expect("Supervisor should not be dead");
        for (_, actor) in &mut self.actors {
            if !actor.actor.is_alive() {
                let timing = calculate_restart(&actor.restart_info);
                match timing {
                    Err(e) => {
                        // Well, that's that then.
                        error!("Too many failures; shutting down.");
                        return Ok(ControlFlow::Break(reason));
                    }
                    Ok(timing) => {
                        // This pauses the supervisor for a while...
                        // TODO: Be smarter.
                        info!(
                            "Sleeping for {} seconds before restart",
                            timing.sleep.as_secs()
                        );
                        sleep(timing.sleep).await;
                        actor.restart_info = timing;
                    }
                }

                actor.actor.restart(&self_reference).await;
            }
        }

        Ok(ControlFlow::Continue(()))
    }
}

impl Message<ReloadConfig> for Supervisor {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: ReloadConfig,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // Reload the configuration.
        let new_config = crate::config::load().context("while reloading configuration")?;
        if new_config == self.config {
            info!("Configuration unchanged, no action taken");
            return Ok(());
        } else {
            info!("Configuration changed, applying new settings");
            self.config = new_config;
            // Apply the new configuration.
            ctx.actor_ref().tell(ApplyConfig).try_send()?;
            Ok(())
        }
    }
}

impl Message<ApplyConfig> for Supervisor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ApplyConfig,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // Anything that's running but shouldn't be, we stop.
        let expected_actors: HashSet<ConfigHash> = self
            .config
            .irc
            .iter()
            .map(|irc_config| (hash(irc_config)))
            .collect();
        let running = self.actors.keys().cloned().collect::<HashSet<ConfigHash>>();
        let killer = running.difference(&expected_actors).map(async |v| {
            let victim = &self.actors[v];
            victim.actor.stop().await;
        });
        join_all(killer).await;

        // Anything that isn't running, but should be, we start.
        let self_ref = ctx.actor_ref();
        let now = Instant::now();
        for irc_config in &self.config.irc {
            let h = hash(irc_config);
            if running.contains(&h) {
                continue;
            }
            let irc_actor = IrcActor::spawn_link(&self_ref, irc_config.clone()).await;
            self.actors.insert(
                h,
                Supervised {
                    restart_info: RestartInfo {
                        failure_count: 0,
                        last_failure: now,
                        first_failure: now,
                        sleep: Duration::from_secs(0),
                    },
                    actor: SomeActor::Irc((irc_config.clone(), irc_actor)),
                },
            );
        }
    }
}

impl SomeActor {
    async fn stop(&self) {
        match self {
            Self::Irc((_, reference)) => {
                let _ = reference.stop_gracefully().await;
                reference.wait_for_shutdown().await;
            }
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Irc((_, reference)) => reference.is_alive(),
        }
    }

    async fn restart(&mut self, link: &ActorRef<Supervisor>) {
        match self {
            Self::Irc((config, reference)) => {
                // Should already be dead, but nevertheless.
                reference.kill();
                let new_reference = IrcActor::spawn_link(link, config.clone()).await;
                *self = Self::Irc((config.clone(), new_reference))
            }
        }
    }
}

fn hash(val: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    val.hash(&mut hasher);
    hasher.finish()
}

fn calculate_restart(info: &RestartInfo) -> Result<RestartInfo> {
    let now = Instant::now();

    // If it's been more than 30 minutes since the last failure, treat this as failure #1
    if now.duration_since(info.last_failure) > Duration::from_secs(1800) {
        return Ok(RestartInfo {
            failure_count: 1,
            last_failure: now,
            first_failure: now,
            sleep: Duration::from_secs(5),
        });
    }

    // Otherwise, increment failure count and update last failure
    let new_failure_count = info.failure_count + 1;

    // If we've seen more than 10 failures in a row, bail
    if new_failure_count > 10 {
        bail!("Too many consecutive failures: {}", new_failure_count);
    }

    // Calculate sleep duration using Fibonacci sequence with 5 second base
    // Sequence: 5, 5, 10, 15, 25, 40, 60, 60, ...
    let sleep_duration = match new_failure_count {
        1 => Duration::from_secs(5),
        2 => Duration::from_secs(5),
        3 => Duration::from_secs(10),
        4 => Duration::from_secs(15),
        5 => Duration::from_secs(25),
        6 => Duration::from_secs(40),
        _ => Duration::from_secs(60), // Cap at 1 minute
    };

    Ok(RestartInfo {
        failure_count: new_failure_count,
        last_failure: now,
        first_failure: info.first_failure,
        sleep: sleep_duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_calculate_restart_reset_after_30_minutes() {
        let now = Instant::now();
        let thirty_one_minutes_ago = now - Duration::from_secs(1860); // 31 minutes

        let info = RestartInfo {
            failure_count: 5,
            last_failure: thirty_one_minutes_ago,
            first_failure: thirty_one_minutes_ago,
            sleep: Duration::from_secs(25),
        };

        let result = calculate_restart(&info).unwrap();

        assert_eq!(result.failure_count, 1);
        assert_eq!(result.sleep, Duration::from_secs(5));
        assert!(result.last_failure > thirty_one_minutes_ago);
        assert!(result.first_failure > thirty_one_minutes_ago);
    }

    #[test]
    fn test_calculate_restart_increment_within_30_minutes() {
        let now = Instant::now();
        let ten_minutes_ago = now - Duration::from_secs(600); // 10 minutes
        let first_failure = now - Duration::from_secs(1200); // 20 minutes ago

        let info = RestartInfo {
            failure_count: 2,
            last_failure: ten_minutes_ago,
            first_failure,
            sleep: Duration::from_secs(5),
        };

        let result = calculate_restart(&info).unwrap();

        assert_eq!(result.failure_count, 3);
        assert_eq!(result.sleep, Duration::from_secs(10));
        assert!(result.last_failure > ten_minutes_ago);
        assert_eq!(result.first_failure, first_failure);
    }

    #[test]
    fn test_calculate_restart_fibonacci_sequence() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(60);
        let first_failure = now - Duration::from_secs(300);

        let test_cases = vec![
            (0, 1, 5),   // failure 1 -> 5 seconds
            (1, 2, 5),   // failure 2 -> 5 seconds
            (2, 3, 10),  // failure 3 -> 10 seconds
            (3, 4, 15),  // failure 4 -> 15 seconds
            (4, 5, 25),  // failure 5 -> 25 seconds
            (5, 6, 40),  // failure 6 -> 40 seconds
            (6, 7, 60),  // failure 7 -> 60 seconds (capped)
            (7, 8, 60),  // failure 8 -> 60 seconds (capped)
            (8, 9, 60),  // failure 9 -> 60 seconds (capped)
            (9, 10, 60), // failure 10 -> 60 seconds (capped)
        ];

        for (initial_count, expected_count, expected_sleep) in test_cases {
            let info = RestartInfo {
                failure_count: initial_count,
                last_failure: recent,
                first_failure,
                sleep: Duration::from_secs(0),
            };

            let result = calculate_restart(&info).unwrap();

            assert_eq!(result.failure_count, expected_count);
            assert_eq!(result.sleep, Duration::from_secs(expected_sleep));
        }
    }

    #[test]
    fn test_calculate_restart_bail_after_10_failures() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(60);
        let first_failure = now - Duration::from_secs(600);

        let info = RestartInfo {
            failure_count: 10,
            last_failure: recent,
            first_failure,
            sleep: Duration::from_secs(60),
        };

        let result = calculate_restart(&info);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Too many consecutive failures: 11")
        );
    }

    #[test]
    fn test_calculate_restart_just_under_30_minutes() {
        let just_under_30_minutes_ago = Instant::now() - Duration::from_secs(1799); // 29 minutes 59 seconds

        let info = RestartInfo {
            failure_count: 3,
            last_failure: just_under_30_minutes_ago,
            first_failure: just_under_30_minutes_ago,
            sleep: Duration::from_secs(10),
        };

        let result = calculate_restart(&info).unwrap();

        // Should increment since it's under 30 minutes
        assert_eq!(result.failure_count, 4);
        assert_eq!(result.sleep, Duration::from_secs(15));
        assert_eq!(result.first_failure, just_under_30_minutes_ago);
    }

    #[test]
    fn test_calculate_restart_first_failure() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(60);
        let first_failure = now - Duration::from_secs(120);

        let info = RestartInfo {
            failure_count: 0,
            last_failure: recent,
            first_failure,
            sleep: Duration::from_secs(0),
        };

        let result = calculate_restart(&info).unwrap();

        assert_eq!(result.failure_count, 1);
        assert_eq!(result.sleep, Duration::from_secs(5));
        assert_eq!(result.first_failure, first_failure);
    }
}
