use std::time::{Duration, Instant};

/// Simple token bucket rate limiter.
#[derive(Debug)]
pub struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    refill_rate: f64, // tokens per second
    max_tokens: f64,
}

impl TokenBucket {
    pub fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            last_refill: Instant::now(),
            refill_rate,
            max_tokens,
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }

    /// Blocks until a single token is available, then consumes it.
    pub async fn consume_token(&mut self) {
        loop {
            if self.try_consume(1.0) {
                return;
            }

            let tokens_needed = 1.0 - self.tokens;
            let wait_time = if self.refill_rate <= f64::EPSILON {
                Duration::from_secs(1)
            } else {
                Duration::from_secs_f64(tokens_needed / self.refill_rate)
            };

            tokio::time::sleep(wait_time).await;
        }
    }

    /// Attempts to consume the requested amount immediately without waiting.
    pub fn try_consume(&mut self, amount: f64) -> bool {
        self.refill();

        if self.tokens >= amount {
            self.tokens -= amount;
            true
        } else {
            false
        }
    }

    /// Forcefully deducts tokens, even if the bucket is empty, without waiting.
    pub fn force_consume(&mut self, amount: f64) {
        self.refill();
        self.tokens = (self.tokens - amount).clamp(0.0, self.max_tokens);
    }
}
