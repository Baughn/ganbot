# Action Broker Refactor Plan

## Objectives
- Decouple network actors (IRC/Discord) from long-running action execution so they remain stateless and resilient across restarts.
- Persist action requests in a Redis sorted set (`actions:pending`) so in-flight work survives process crashes.
- Introduce a global broker actor that mediates action lifecycle, spawning dedicated workers and routing updates back to the originating network actor.

## Architecture Outline
1. **Action Model**
   - Define `Action` enum in `src/actions.rs` encapsulating command inputs (`Ask`, `Combine`, `Prompt`, `Dream`, etc.) and an `Origin` descriptor (`IRC`, `Discord`).
   - Include `action_id`, `requested_at`, `retry_count`, and origin metadata required to reconstruct responses after restarts.
   - Add helper serialization/deserialization with `serde` to store in Redis.

2. **Broker Actor**
   - New `ActionBroker` singleton actor responsible for:
     - Accepting `SubmitAction` messages from network actors.
     - Storing action records in Redis sorted set (`actions:pending`).
     - Maintaining an in-memory registry mapping `Origin` → live `ActorRef` for reply delivery.
     - Buffering outbound updates when a network actor is temporarily unavailable (stash to Redis list like `actions:notifications:<origin>`).

3. **Worker Flow**
   - Broker immediately spawns a short-lived worker per request and tracks it in Redis until completion.
   - On restart, broker rechurns any entries still present in the sorted set and replays `Queued` updates before respawning workers.
   - Future retry/backoff logic can still re-enqueue requests with incremented `retry_count`.

4. **Feedback Integration**
   - Modify broker to expose `PublishProgress` API used by ComfyUI listener or lifecycle actors.
   - Broker forwards updates to registered origins (retrying with jitter, falling back to Redis notification stash if dest actor missing).

5. **Network Actor Changes**
   - IRC/Discord actors create an `Origin` token on command receipt and call `SubmitAction` instead of spawning commands directly.
   - Implement registration handshake on startup (`RegisterOrigin { origin_key, actor_ref }`).
   - Add logic to drain pending notifications on startup for Discord/IRC recovery.

6. **Persistence / Types**
   - Extend configuration or supervisor init to start `ActionBroker`, handing it the shared Redis connection.
   - Ensure all new types derive `Serialize`, `Deserialize`, and respect existing error handling patterns (`anyhow::Result`).

## Implementation Steps
1. Add `Action`, `ActionPayload`, and `Origin` types in `src/actions.rs`, alongside message structs for broker interaction.
2. Create `src/actions/broker.rs` (or similar) with `ActionBroker` plus any helper structs, alongside Redis sorted-set helpers.
3. Wire broker startup into `Supervisor::on_start`, register actors in `ACTOR_REGISTRY`, and provide accessor similar to `UserManager::get()`.
4. Refactor IRC command handling to submit actions via broker and rely on status updates for responses; adjust imports accordingly.
5. Introduce (stub) Discord integration points mirroring IRC behavior for future use.
6. Add Redis sorted-set helpers as needed; ensure `cargo fmt` passes.
7. Run `cargo fmt` and `cargo clippy -- -D warnings`; fix compile issues iteratively.

## Open Questions / Follow-ups
- Decide exact schema for Redis notification backlog (list vs stream) and pruning strategy.
- Determine how ComfyUI feedback actor authenticates messages with `action_id`.
- Evaluate whether to persist status snapshots beyond Discord message IDs for long-term auditing.

