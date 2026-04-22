# Ganbot5 architecture sketch: Tower services + Kameo actors

> A design sketch for a hypothetical successor to ganbot4, intended to be
> read standalone. Written as a prompt for discussion, not a fixed plan.

## Thesis

Ganbot4 leans on Kameo for everything, and most of the resulting actors
don't own state worth isolating. They're async functions wrapped in an
actor suit, reachable through a global `ACTOR_REGISTRY`. That pattern
adds indirection without paying for it.

Ganbot5 draws the line differently:

- **Tower services** for external integrations (OpenRouter, ComfyUI,
  image host). These are stateless-ish, concurrency-friendly, and
  compose beautifully with middleware (retry, cache, rate-limit, trace).
- **Kameo actors** for things that genuinely have state or a lifecycle:
  the IRC connection, per-user state, per-conversation memory, the
  supervisor. Fewer actors, each earning its existence.
- **Plain async functions** for command handlers. They borrow an
  `AppContext` carrying service handles and actor refs, and call out to
  services/actors as needed. No actor spawn per command invocation.

The payoff: less boilerplate, better testability, an obvious home for
RAG-backed conversational memory, and middleware you can compose instead
of reimplement.

---

## Layer map

```
┌─────────────────────────────────────────────────────────────────┐
│  Entry points: IrcClient, DiscordClient, WebServer              │
│  (Kameo actors — own connections, emit inbound events)          │
└───────────────────┬─────────────────────────────────────────────┘
                    │ inbound events
                    ▼
┌─────────────────────────────────────────────────────────────────┐
│  ConversationActor  (per user+channel, Kameo)                   │
│  - message history, RAG handle, tool-call state, mode           │
│  - decides what to do with an inbound message                   │
└───────────────────┬─────────────────────────────────────────────┘
                    │ dispatches to
                    ▼
┌─────────────────────────────────────────────────────────────────┐
│  Command handlers  (plain async fn)                             │
│  handle_prompt, handle_combine, handle_ask, handle_edit, …      │
└───────────────────┬─────────────────────────────────────────────┘
                    │ calls through AppContext
                    ▼
┌─────────────────────────────────────────────────────────────────┐
│  Tower services                                                 │
│  OpenRouterService, ComfyUIService, ImageHostService, RagService│
│  composed with retry/cache/rate-limit/trace layers              │
└─────────────────────────────────────────────────────────────────┘
```

Kameo only at the top and middle. Tower at the bottom. Plain functions
in between.

---

## `AppContext`: the dependency bundle

One struct, constructed once in `main`, cheaply clonable, handed to every
command handler and non-trivial actor on spawn. Replaces the
`ACTOR_REGISTRY` singleton and the `fn get() -> ActorRef<X>` pattern.

```rust
#[derive(Clone)]
pub struct AppContext {
    // Tower services — clone-cheap (Arc inside)
    pub openrouter:  OpenRouterService,
    pub comfyui:     ComfyUIService,
    pub image_host:  ImageHostService,
    pub rag:         RagService,

    // Actor refs — also cheap to clone
    pub user_manager: ActorRef<UserManager>,
    pub supervisor:   ActorRef<Supervisor>,

    // Shared immutable config / state
    pub models:  Arc<ModelsConfig>,
    pub redis:   ConnectionManager,
}
```

Tests construct an `AppContext` with fake services (`tower::service_fn`)
and synthetic actor refs. No global state, no locks, no "forgot to
register in main."

---

## Services

### OpenRouter as a Tower service

```rust
// Request/response types
pub struct CompletionRequest {
    pub origin: String,
    pub models: Vec<String>,   // fallback chain
    pub prompt: String,
    pub image:  Option<RequestImage>,
    pub expect_image: bool,
    pub schema: Option<JsonSchemaConfig>,
}

pub struct CompletionResponse {
    pub model: Option<String>,
    pub text:  Option<String>,
    pub image: Option<RgbImage>,
}

// The service itself
#[derive(Clone)]
pub struct OpenRouterService {
    inner: Arc<OpenRouterInner>,
}

struct OpenRouterInner {
    client: reqwest::Client,
    token:  String,
}

impl Service<CompletionRequest> for OpenRouterService {
    type Response = CompletionResponse;
    type Error    = OpenRouterError;
    type Future   = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))  // always ready; rate limiting comes via a layer
    }

    fn call(&mut self, req: CompletionRequest) -> Self::Future {
        let inner = self.inner.clone();
        Box::pin(async move { inner.request(req).await })
    }
}
```

Wrapped with layers at construction time:

```rust
fn build_openrouter(cfg: &OpenrouterConfig) -> OpenRouterService {
    let base = OpenRouterService::new(cfg.token.clone());

    ServiceBuilder::new()
        .layer(TraceLayer::new("openrouter"))
        .layer(TimeoutLayer::new(Duration::from_secs(120)))
        .layer(RateLimitLayer::new(60, Duration::from_secs(60)))
        .layer(RetryLayer::new(ExponentialBackoff::default().with_max_retries(3)))
        .layer(CacheLayer::by_hash(Duration::from_secs(300)))  // idempotent prompts
        .service(base)
        .into()
}
```

The layer stack is the place *all* the cross-cutting concerns live. Today
these are scattered (retry in one actor, rate-limiting implicit, no
caching, tracing ad-hoc). One stack, readable top-down, testable
individually.

### ComfyUI the same shape

```rust
pub struct GenerateRequest {
    pub params:   ComfyParams,
    pub progress: Option<mpsc::Sender<Progress>>,  // optional live updates
    pub cancel:   CancellationToken,
}

pub struct GenerateResponse {
    pub images:    Vec<RgbImage>,
    pub workflow:  serde_json::Value,
    pub seed:      Option<u64>,
}

impl Service<GenerateRequest> for ComfyUIService { /* websocket pool */ }
```

Progress streams stay outside the request/response type by passing a
channel in. The service doesn't need to know anything about IRC or how
progress is displayed.

### ImageHostService

Upload → URL. One method. Clean boundary; easy to swap (S3, local,
brage.info SSH) without touching callers.

### RagService

New, for RAG-backed memory. `Service<EmbedRequest>`,
`Service<SearchRequest>`. Backed by SQLite+vec0 or `hnsw_rs` to start —
embed text, store, retrieve similar. The `ConversationActor` is its
primary client.

---

## Actors that survive

### `IrcClient`

Same role as today, but thinner. Owns the `irc::Client` connection,
handles the outbound queue (rate limiting, message batching, 400-byte
splits), parses inbound messages, and forwards to a routing layer that
dispatches to the right `ConversationActor`.

```rust
pub struct IrcClient {
    config:    IrcConfig,
    conn:      irc::Client,
    outbound:  mpsc::Receiver<OutboundIrc>,
    inbound:   mpsc::Sender<InboundMessage>,
    rate:      Governor,
}

impl Actor for IrcClient { /* reconnect logic, main loop */ }
```

Commands are *not* handled here. This actor only speaks IRC. Inbound
events go out on a channel, consumed by a thin router.

### `UserManager` and `UserActor`

One `UserActor` per active user, lazily spawned, cached by
`UserManager`. Owns: aliases, default prompt, selected image, generated
image history. Per-user serialization (message ordering) for free, which
matters for mutation.

### `ConversationActor` (the new and actually-useful one)

Spawned lazily per `(user, channel)` pair, kept alive as long as the
conversation is active (idle timeout, e.g. 30 min).

Owns:

- **Message history** — rolling window, persisted to Redis.
- **RAG index handle** — embeds each user/bot turn, retrieves relevant
  context on incoming messages.
- **Pending tool-call state** — if an image-generation job is running,
  the actor holds the cancellation token and the "waiting on result"
  state; a follow-up message from the user can amend or cancel.
- **Mode** — is a combine game active? an image-edit session? what
  model are we defaulting to?

This is where actor isolation earns its keep. Concurrency story:
"messages from user X in channel Y are serialized, one at a time,
through one actor." No locks, no races, and the long-lived state
(RAG, history) lives naturally alongside the logic.

```rust
pub struct ConversationActor {
    ctx:         AppContext,
    user:        UserId,
    channel:     ChannelId,
    history:     RollingHistory,
    mode:        ConversationMode,
    active_job:  Option<ActiveJob>,
}

enum ConversationMode {
    Idle,
    CombineGame { state: CombineState },
    ImageEdit   { session: EditSession },
}

impl Message<InboundMessage> for ConversationActor {
    async fn handle(&mut self, msg: InboundMessage, _: &mut Context<Self, _>) -> Result<()> {
        // Append to history, embed, update RAG.
        self.history.push(Turn::user(msg.text.clone()));
        self.ctx.rag.clone().oneshot(EmbedRequest::new(&msg.text)).await?;

        // Dispatch based on mode and message.
        match parse_command(&msg.text) {
            Some(Command::Prompt(args)) => {
                let reply = handle_prompt(&self.ctx, &self.user, args).await?;
                self.history.push(Turn::bot(reply.text.clone()));
                send_reply(&self.ctx, &self.channel, reply).await?;
            }
            Some(Command::Edit(args)) => {
                self.mode = self.start_edit_session(args).await?;
            }
            None if self.should_converse(&msg) => {
                let reply = handle_ask_with_memory(&self.ctx, &self.history, &msg).await?;
                self.history.push(Turn::bot(reply.clone()));
                send_reply(&self.ctx, &self.channel, PlainText(reply)).await?;
            }
            _ => {}
        }
        Ok(())
    }
}
```

### `Supervisor`

Skinnier than today. Owns restart policy for the few remaining actors,
watches config files, coordinates shutdown. No longer the dependency
injection point — that's `AppContext`.

---

## Command handlers: plain async fns

No more `CombineActor`, no more `ImagenActor`, no more spawning an actor
per command invocation. Just functions.

```rust
pub async fn handle_prompt(
    ctx:  &AppContext,
    user: &UserId,
    args: PromptArgs,
) -> Result<PromptReply> {
    let model = resolve_model(&ctx.models, args.model.as_deref())?;

    let images = match &model.backend {
        Backend::OpenRouter { model: or_model } => {
            let req = CompletionRequest {
                origin: format!("prompt:{user}"),
                models: vec![or_model.clone()],
                prompt: args.prompt.clone(),
                expect_image: true,
                ..Default::default()
            };
            let resp = ctx.openrouter.clone().oneshot(req).await?;
            resp.image.into_iter().collect()
        }
        Backend::ComfyUI { params, .. } => {
            let req = GenerateRequest { params: params.clone(), progress: None, cancel: Default::default() };
            let resp = ctx.comfyui.clone().oneshot(req).await?;
            resp.images
        }
    };

    let urls = upload_all(&ctx.image_host, &images).await?;

    ctx.user_manager
        .ask(RecordGeneration { user: user.clone(), urls: urls.clone(), model: model.name.clone() })
        .await?;

    Ok(PromptReply { urls, text: None })
}
```

Testable without any actor framework: build an `AppContext` with
`service_fn` services that return canned data, call the fn, assert on
the result. No `spawn`, no lifecycle, no weird async fixture setup.

---

## Middleware as reusable Tower layers

A few that ganbot4 currently bakes in ad-hoc:

- **`RetryLayer`** — `tower::retry` with `ExponentialBackoff`. Replaces
  the hand-rolled retry in `generate_single_nanobanana_image`.
- **`RateLimitLayer`** — `tower::limit::rate` or a `governor`-backed
  custom layer. Replaces the implicit "we only make one OpenRouter call
  at a time" pattern.
- **`CacheLayer`** — keyed by a hash of the request. Replaces the
  `combine:combinations` Redis hash. Backend-agnostic: LRU in-process or
  Redis-backed depending on construction.
- **`TimeoutLayer`** — `tower::timeout`. Every external call gets one.
- **`TraceLayer`** — tracing span per request with model, duration,
  status. Replaces ad-hoc `info!("...")` calls.
- **`ModelFallbackLayer`** — custom. If the primary model returns 5xx or
  a specific error class, retry with the next model in the chain. Replaces
  today's `select_models` merging logic cleanly.

Each layer is ~20–50 lines and individually testable.

---

## Testing story

- **Services**: construct with `service_fn(|req| async { Ok(fake_response) })`.
  Layers are tested by wrapping a `service_fn` probe and asserting what
  requests reach it.
- **Actors**: Kameo lets you `spawn` them inline; tests send messages
  directly. Pass an `AppContext` built from `service_fn`s.
- **Command handlers**: plain async fns, plain `#[tokio::test]`.
- **Integration**: a harness that boots the whole `AppContext` against
  `wiremock` for HTTP and an in-memory Redis (`redis-test`), then drives
  it via synthetic IRC messages.

---

## Tradeoffs and open questions

### Tower's generics ergonomics

`ServiceBuilder` stacks produce ugly types. Two mitigations:

1. **`BoxService` at field boundaries.** `AppContext.openrouter` is a
   `BoxService<CompletionRequest, CompletionResponse, OpenRouterError>`
   — one `Box<dyn>` indirection per call, small cost, hugely readable
   code.
2. **Type aliases.** `type OpenRouterService = BoxCloneService<...>;` in
   one place, everything downstream references the alias.

Either way you opt out of monomorphisation at the context boundary.
Good trade; compile times stay reasonable, error messages stay legible.

### Kameo vs. raw channels

If the actor count stays small (IRC × N, UserManager, Supervisor,
ConversationActor × active conversations), raw `tokio::sync::mpsc`
channels might be enough. Kameo earns its keep mainly for supervision
and typed `ask`. It's a close call; I'd lean Kameo for the `ask` reply
type safety and the restart machinery, but don't feel obligated.

### Where does image editing live?

Two options:

1. **`ImageEditSession` sub-state of `ConversationActor`.** The actor
   holds the in-progress image, the instruction chain, the model. Simple;
   one place owns it.
2. **Separate `EditSessionActor` spawned by ConversationActor.** Useful
   if edits are long-running and you want to cancel conversationally.

Either works. Start with (1), promote to (2) if it grows.

### RAG backend choice

- **SQLite + `sqlite-vec`**: single file, no ops, in-process, fast
  enough for the scale of an IRC bot.
- **`hnsw_rs`**: pure Rust, fully in-process, no persistence story out
  of the box.
- **Qdrant**: separate service, overkill unless you're doing something
  fancy across multiple bots.

Default to SQLite+vec until the numbers demand otherwise.

### Do you want structured concurrency primitives?

Consider `tokio_util::task::TaskTracker` or `JoinSet` inside the
`ConversationActor` for spawning short-lived child tasks (an image job,
a RAG query) with clean cancellation. Kameo doesn't provide this
directly but composes with it fine.

---

## Migration shape for the new repo

Order that keeps each step shippable:

1. **`AppContext` skeleton + config loading.** No services yet, just the
   struct and a `main` that builds it.
2. **`OpenRouterService`** as a bare Tower service (no layers), plus the
   layers one at a time behind it. Unit tests per layer.
3. **`IrcClient` actor**, inbound channel, a trivial router that prints
   messages. Proves the actor boundary.
4. **A single command: `!ask`.** Plain async fn, talks to OpenRouter,
   replies via IRC. End-to-end slice.
5. **`UserManager` + `UserActor`.** Bring in Redis persistence.
6. **`ComfyUIService`** + `!prompt`. Second command, exercises the "dispatch
   on backend" pattern.
7. **`ConversationActor`** without RAG. Just history + message routing.
   The existence of the actor unlocks per-conversation state.
8. **`RagService`** + wire it into `ConversationActor`. First time the
   actor pays back its existence.
9. **Image editing** as a `ConversationMode`. Demonstrates that modes +
   actor state handle multi-turn flows cleanly.

Each step is a few days of work and produces something runnable. No big
bang.

---

## What this deliberately does *not* do

- No actor per command invocation.
- No `ACTOR_REGISTRY` / global singleton lookups.
- No message types scattered across `messages/` and inline; keep them
  next to the service or actor that owns them.
- No ad-hoc retry loops inside request functions.
- No "config carries both `image_model` and per-model backend data"
  duplication (ganbot4's original sin on this refactor).
