pub async fn retrying<F, Fut, T, E, P>(
    attempts: usize,
    should_retry: P,
    mut operation: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    P: Fn(&E) -> bool,
{
    let mut last_error = None;

    for _ in 0..attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) if should_retry(&err) => {
                last_error = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_error.unwrap())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn retryable_error_is_attempted_exactly_the_configured_number_of_times() {
        let attempts = AtomicUsize::new(0);

        let result: Result<(), &str> = retrying(
            3,
            |_: &&str| true,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Err("nope") }
            },
        )
        .await;

        assert_eq!(result, Err("nope"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_error_returns_after_a_single_attempt() {
        let attempts = AtomicUsize::new(0);

        let result: Result<(), &str> = retrying(
            3,
            |_: &&str| false,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Err("nope") }
            },
        )
        .await;

        assert_eq!(result, Err("nope"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn success_on_a_later_attempt_stops_retrying() {
        let attempts = AtomicUsize::new(0);

        let result: Result<&str, &str> = retrying(
            3,
            |_: &&str| true,
            || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 1 {
                        Ok("done")
                    } else {
                        Err("nope")
                    }
                }
            },
        )
        .await;

        assert_eq!(result, Ok("done"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
