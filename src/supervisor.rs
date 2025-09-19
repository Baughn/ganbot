use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher as _};
/// Root of the supervision tree.
/// This module contains the main logic for managing the application's lifecycle,
/// including starting and stopping modules, handling configuration changes,
/// and managing global state.
use std::ops::ControlFlow;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::actions::broker::ActionBroker;
use anyhow::{Context as _, Result, bail};
use futures::future::join_all;
use kameo::error::Infallible;
use kameo::mailbox;
use kameo::prelude::*;
use kameo::registry::ACTOR_REGISTRY;
use kameo::{Actor, actor::ActorRef};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{error, info, instrument};

use crate::config::{
    Config,
    global::{DiscordConfig, ImageHostConfig, IrcConfig, OpenrouterConfig},
    models::{ModelsConfig, load_models_config},
};
use crate::network::discord::DiscordActor;
use crate::network::irc::IrcActor;
use crate::network::openrouter::OpenRouter;
use crate::persistence::user::UserManager;

type ConfigHash = u64;

const WATCHED_CONFIG_FILES: &[&str] = &["config.toml", "config-local.toml", "models.toml"];

pub struct Supervisor {
    /// Currently active configuration.
    config: Config,
    /// Models configuration.
    models_config: ModelsConfig,
    /// Actors implied by said configuration.
    actors: HashMap<ConfigHash, Supervised>,
    /// Redis connection.
    redis_connection: redis::aio::ConnectionManager,
    /// User manager.
    _user_manager: ActorRef<UserManager>,
    /// Action broker.
    _action_broker: ActorRef<ActionBroker>,
    /// Filesystem watcher for configuration changes (kept alive for duration of actor).
    _config_watcher: Option<RecommendedWatcher>,
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

/// The necessary information to restart a supervisor-managed actor.
/// These are precisely the actors that need to be restarted on a config change.
enum SomeActor {
    Irc((IrcConfig, ActorRef<IrcActor>)),
    OpenRouter((OpenrouterConfig, ActorRef<OpenRouter>)),
    Discord((DiscordConfig, ActorRef<DiscordActor>)),
}

struct Supervised {
    restart_info: RestartInfo,
    actor: SomeActor,
}

struct ReloadConfig;
struct GetRedis;
#[derive(Reply)]
struct GetRedisReply(redis::aio::ConnectionManager);
struct GetImageHost;
#[derive(Reply)]
struct GetImageHostReply(ImageHostConfig);
pub struct GetModelsConfig;
#[derive(Reply)]
pub struct GetModelsConfigReply(pub ModelsConfig);

impl Actor for Supervisor {
    type Args = Config;
    type Error = Infallible;

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        actor_ref.register("supervisor").unwrap();
        // Connect to Redis
        let client =
            redis::Client::open(args.redis_url.as_str()).expect("Failed to create Redis client");
        let redis_connection = client
            .get_connection_manager()
            .await
            .expect("Failed to connect to Redis");
        tokio::spawn(redis_keepalive(redis_connection.clone()));
        // Initialize the user manager.
        let user_manager = UserManager::spawn_link(&actor_ref, redis_connection.clone()).await;
        let action_broker = ActionBroker::spawn_link(&actor_ref, redis_connection.clone()).await;

        // Load models configuration
        let models_config = load_models_config().expect("Failed to load models configuration");

        let _config_watcher = match start_config_watcher(actor_ref.clone()) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                error!(?error, "Failed to start configuration watcher");
                None
            }
        };

        let mut supervisor = Supervisor {
            config: args,
            models_config,
            actors: HashMap::new(),
            redis_connection,
            _user_manager: user_manager,
            _action_broker: action_broker,
            _config_watcher,
        };

        supervisor.apply_config(&actor_ref).await;

        Ok(supervisor)
    }

    #[instrument(skip(self, actor_ref, reason))]
    async fn on_link_died(
        &mut self,
        actor_ref: WeakActorRef<Self>,
        id: ActorId,
        reason: ActorStopReason,
    ) -> std::result::Result<ControlFlow<ActorStopReason>, Self::Error> {
        // Check for dead actors & restart them.
        let self_reference = actor_ref.upgrade().expect("Supervisor should not be dead");
        for actor in self.actors.values_mut() {
            if !actor.actor.is_alive() {
                let timing = calculate_restart(&actor.restart_info);
                match timing {
                    Err(e) => {
                        // Well, that's that then.
                        error!("Too many failures; shutting down: {e}");
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

impl Supervisor {
    async fn apply_config(&mut self, actor_ref: &ActorRef<Supervisor>) {
        let mut expected_actors: HashSet<ConfigHash> = self.config.irc.iter().map(hash).collect();

        for discord_config in &self.config.discord {
            expected_actors.insert(hash(discord_config));
        }

        if !self.config.openrouter.token.is_empty() {
            expected_actors.insert(hash(&self.config.openrouter));
        }

        let running = self.actors.keys().cloned().collect::<HashSet<ConfigHash>>();
        let actors_to_stop: Vec<ConfigHash> =
            running.difference(&expected_actors).cloned().collect();

        let killer = actors_to_stop
            .iter()
            .filter_map(|id| self.actors.get(id))
            .map(|victim| async {
                victim.actor.stop().await;
            });
        join_all(killer).await;

        for actor_id in &actors_to_stop {
            self.actors.remove(actor_id);
        }

        let running = self.actors.keys().cloned().collect::<HashSet<ConfigHash>>();
        let self_ref = actor_ref.clone();
        let now = Instant::now();

        if !self.config.openrouter.token.is_empty() {
            let h = hash(&self.config.openrouter);
            if !running.contains(&h) {
                let openrouter_actor =
                    OpenRouter::spawn_link(&self_ref, self.config.openrouter.clone()).await;

                self.actors.insert(
                    h,
                    Supervised {
                        restart_info: RestartInfo {
                            failure_count: 0,
                            last_failure: now,
                            first_failure: now,
                            sleep: Duration::from_secs(0),
                        },
                        actor: SomeActor::OpenRouter((
                            self.config.openrouter.clone(),
                            openrouter_actor,
                        )),
                    },
                );
            }
        }

        for discord_config in &self.config.discord {
            let h = hash(discord_config);
            if running.contains(&h) {
                continue;
            }
            let discord_actor = DiscordActor::spawn_link(&self_ref, discord_config.clone()).await;
            self.actors.insert(
                h,
                Supervised {
                    restart_info: RestartInfo {
                        failure_count: 0,
                        last_failure: now,
                        first_failure: now,
                        sleep: Duration::from_secs(0),
                    },
                    actor: SomeActor::Discord((discord_config.clone(), discord_actor)),
                },
            );
        }

        for irc_config in &self.config.irc {
            let h = hash(irc_config);
            if running.contains(&h) {
                continue;
            }
            let irc_actor = IrcActor::spawn_link_with_mailbox(
                &self_ref,
                irc_config.clone(),
                mailbox::unbounded(),
            )
            .await;
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

/// Periodically send PING to Redis to keep the connection alive.
async fn redis_keepalive(redis: redis::aio::ConnectionManager) {
    loop {
        let _: Result<String, _> = redis::cmd("PING").query_async(&mut redis.clone()).await;
        sleep(Duration::from_secs(60)).await;
    }
}

fn start_config_watcher(actor_ref: ActorRef<Supervisor>) -> notify::Result<RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                let targets_change = event.paths.iter().any(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| WATCHED_CONFIG_FILES.contains(&name))
                        .unwrap_or(false)
                });

                if targets_change && tx.send(event).is_err() {
                    info!("Configuration watcher receiver dropped; stopping notifications");
                }
            }
            Err(error) => error!(?error, "Configuration watcher encountered an error"),
        })?;

    watcher.watch(Path::new("."), RecursiveMode::NonRecursive)?;

    tokio::spawn(async move {
        let mut last_reload: Option<Instant> = None;
        while let Some(event) = rx.recv().await {
            if !matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                continue;
            }

            let now = Instant::now();
            if let Some(previous) = last_reload {
                if now.duration_since(previous) < Duration::from_millis(250) {
                    continue;
                }
            }
            last_reload = Some(now);

            info!(?event.kind, "Configuration change detected; requesting reload");

            if let Err(error) = actor_ref.tell(ReloadConfig).try_send() {
                error!(?error, "Failed to send ReloadConfig message");
            }
        }
    });

    Ok(watcher)
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
        let config_changed = new_config != self.config;

        if config_changed {
            info!("Configuration changed, applying new settings");
            self.config = new_config;
            // Apply the new configuration.
            let actor_ref = ctx.actor_ref();
            self.apply_config(&actor_ref).await;
        } else {
            info!("Configuration unchanged, no action taken");
        }

        // Always attempt to reload models config regardless of main config changes
        match load_models_config() {
            Ok(new_models_config) => {
                info!("Successfully reloaded models configuration");
                self.models_config = new_models_config;
            }
            Err(e) => {
                error!(
                    "Failed to reload models configuration, keeping old config: {}",
                    e
                );
            }
        }

        Ok(())
    }
}

impl Message<GetRedis> for Supervisor {
    type Reply = GetRedisReply;

    async fn handle(
        &mut self,
        _msg: GetRedis,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetRedisReply(self.redis_connection.clone())
    }
}

impl Message<GetImageHost> for Supervisor {
    type Reply = GetImageHostReply;

    async fn handle(
        &mut self,
        _msg: GetImageHost,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetImageHostReply(self.config.image_host.clone())
    }
}

impl Message<GetModelsConfig> for Supervisor {
    type Reply = GetModelsConfigReply;

    async fn handle(
        &mut self,
        _msg: GetModelsConfig,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetModelsConfigReply(self.models_config.clone())
    }
}

impl Supervisor {
    /// Get a copy of the Redis connection.
    pub async fn redis() -> redis::aio::ConnectionManager {
        let actor_ref = ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<Supervisor, str>("supervisor")
            .expect("while getting Supervisor actor from registry")
            .expect("Supervisor actor not found in registry");
        actor_ref
            .ask(GetRedis)
            .await
            .expect("while getting Redis connection")
            .0
    }

    /// Get the image host configuration.
    pub async fn image_host() -> ImageHostConfig {
        let actor_ref = ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<Supervisor, str>("supervisor")
            .expect("while getting Supervisor actor from registry")
            .expect("Supervisor actor not found in registry");
        actor_ref
            .ask(GetImageHost)
            .await
            .expect("while getting image host configuration")
            .0
    }

    /// Get the models configuration.
    pub async fn models_config() -> ModelsConfig {
        let actor_ref = ACTOR_REGISTRY
            .lock()
            .unwrap()
            .get::<Supervisor, str>("supervisor")
            .expect("while getting Supervisor actor from registry")
            .expect("Supervisor actor not found in registry");
        actor_ref
            .ask(GetModelsConfig)
            .await
            .expect("while getting models configuration")
            .0
    }
}

impl SomeActor {
    async fn stop(&self) {
        match self {
            Self::Irc((_, reference)) => {
                let _ = reference.stop_gracefully().await;
                reference.wait_for_shutdown().await;
            }
            Self::OpenRouter((_, reference)) => {
                let _ = reference.stop_gracefully().await;
                reference.wait_for_shutdown().await;
            }
            Self::Discord((_, reference)) => {
                let _ = reference.stop_gracefully().await;
                reference.wait_for_shutdown().await;
            }
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Irc((_, reference)) => reference.is_alive(),
            Self::OpenRouter((_, reference)) => reference.is_alive(),
            Self::Discord((_, reference)) => reference.is_alive(),
        }
    }

    async fn restart(&mut self, link: &ActorRef<Supervisor>) {
        match self {
            Self::Irc((config, reference)) => {
                // Should already be dead, but nevertheless.
                reference.kill();
                let new_reference =
                    IrcActor::spawn_link_with_mailbox(link, config.clone(), mailbox::unbounded())
                        .await;
                *self = Self::Irc((config.clone(), new_reference))
            }
            Self::OpenRouter((config, reference)) => {
                reference.kill();
                let new_reference = OpenRouter::spawn_link(link, config.clone()).await;
                *self = Self::OpenRouter((config.clone(), new_reference))
            }
            Self::Discord((config, reference)) => {
                reference.kill();
                let new_reference = DiscordActor::spawn_link(link, config.clone()).await;
                *self = Self::Discord((config.clone(), new_reference))
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
