use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use tokio::sync::{Mutex, broadcast};
use twilight_gateway::{Event as GatewayEvent, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::Client;
use twilight_http::error::ErrorType;
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::channel::message::EmojiReactionType;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker},
};
use uuid::Uuid;

use crate::domain::event::{Codec, OutboxMessage};
use crate::domain::event_bus::EventBus;
use crate::domain::payment::{self, Ack, Error as PaymentError, Event, Payment, Source};
use crate::domain::run::retrying;
use crate::domain::startup_task::StartupTask;
use crate::infra::codec::JsonCodec;
use crate::infra::persistence::sqlx_payment_repository as payment_repo;

const CONFIRMATIONS_CHANNEL: &str = "confirmations";
const APPROVE_EMOJI: &str = "✅";
const REJECT_EMOJI: &str = "❎";
const DELIVERY_ATTEMPTS: usize = 3;
/// The `retrying` attempts above are immediate; this is the actual backoff before the message
/// is handed back to the queue for a fresh consumer poll.
const NACK_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq)]
enum Failure {
    Retry,
    GiveUp,
}

pub struct DiscordConsumer {
    client: Client,
    channels: HashMap<String, String>,
    pool: SqlitePool,
    bus: Arc<dyn EventBus>,
    receiver: Mutex<broadcast::Receiver<OutboxMessage>>,
    nack_delay: Duration,
}

impl DiscordConsumer {
    pub fn new(
        token: String,
        channels: HashMap<String, String>,
        pool: SqlitePool,
        bus: Arc<dyn EventBus>,
    ) -> Self {
        let receiver = bus.subscribe();
        Self {
            client: Client::new(token),
            channels,
            pool,
            bus,
            receiver: Mutex::new(receiver),
            nack_delay: NACK_DELAY,
        }
    }

    fn channel_id(&self, key: &str) -> Option<Id<ChannelMarker>> {
        let raw = self.channels.get(key)?;
        raw.parse::<u64>()
            .inspect_err(|_| tracing::warn!(channel_key = key, raw, "invalid discord channel id"))
            .ok()
            .map(Id::new)
    }

    fn prompt(payment: &Payment) -> String {
        let mut links = String::new();
        if let Source::Email { message_id } = &payment.source {
            links.push_str(&format!("\nEmail: {}", gmail_link(message_id)));
        }
        format!(
            "{}: {} requires confirmation, click {APPROVE_EMOJI} to confirm or {REJECT_EMOJI} to reject{links}",
            payment.details.display_title(),
            payment.total
        )
    }

    async fn remember_message(
        &self,
        payment_id: Uuid,
        channel_id: Id<ChannelMarker>,
        message_id: Id<MessageMarker>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO discord_confirmation_messages
             (confirmation_id, channel_id, message_id) VALUES (?, ?, ?)",
        )
        .bind(payment_id.to_string())
        .bind(channel_id.get() as i64)
        .bind(message_id.get() as i64)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_message(
        &self,
        payment_id: Uuid,
    ) -> sqlx::Result<Option<(Id<ChannelMarker>, Id<MessageMarker>)>> {
        Ok(sqlx::query(
            "SELECT channel_id, message_id FROM discord_confirmation_messages
             WHERE confirmation_id = ?",
        )
        .bind(payment_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(|row| {
            let channel_id: i64 = row.get("channel_id");
            let message_id: i64 = row.get("message_id");
            (Id::new(channel_id as u64), Id::new(message_id as u64))
        }))
    }

    async fn forget_message(&self, payment_id: Uuid) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM discord_confirmation_messages WHERE confirmation_id = ?")
            .bind(payment_id.to_string())
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn handle(&self, message: &OutboxMessage) -> Result<(), Failure> {
        if !matches!(
            message.event_type.as_str(),
            "PAYMENT_CREATED" | "PAYMENT_ACKED"
        ) {
            return Ok(());
        }
        // a poison payload requeued forever is a payload retried forever - give up on it once,
        // loudly, instead of wedging the queue.
        let event: Event = JsonCodec::decode(&message.payload).map_err(|err| {
            tracing::error!(%err, message_id = %message.id, "failed to decode outbox payload");
            Failure::GiveUp
        })?;

        match event {
            Event::Created { id } => self.post_prompt(id).await,
            Event::Acked { id } => self.withdraw(id).await,
            Event::Finished { .. } => Ok(()),
        }
    }

    async fn post_prompt(&self, payment_id: Uuid) -> Result<(), Failure> {
        let payment = match payment_repo::find(&self.pool, payment_id).await {
            Ok(payment) => payment,
            Err(PaymentError::NotFound) => return Ok(()),
            Err(err) => {
                tracing::warn!(%err, %payment_id, "failed to look up payment");
                return Err(Failure::Retry);
            }
        };
        // acked via the UI before delivery (or acked on creation, like manual payments) -
        // nothing left to ask, and this is also what makes out-of-order delivery against
        // `withdraw` safe.
        if payment.ack != Ack::Pending {
            return Ok(());
        }
        // the outbox delivers at least once - a redelivered event must not post a second prompt.
        match self.find_message(payment_id).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(%err, "failed to look up discord message reference");
                return Err(Failure::Retry);
            }
        }

        let Some(channel_id) = self.channel_id(CONFIRMATIONS_CHANNEL) else {
            tracing::warn!("no discord channel configured, will retry");
            return Err(Failure::Retry);
        };

        let content = Self::prompt(&payment);
        let message = match self
            .client
            .create_message(channel_id)
            .content(&content)
            .await
        {
            Ok(response) => match response.model().await {
                Ok(message) => message,
                Err(err) => {
                    tracing::warn!(%err, "failed to decode discord message response");
                    return Err(Failure::Retry);
                }
            },
            // a message Discord will never accept must not wedge the queue behind it - every
            // other failure (rate limit, outage, transient error) is worth retrying.
            Err(err) if matches!(err.kind(), ErrorType::Validation) => {
                tracing::error!(%err, %payment_id, "discord rejected the prompt, giving up");
                return Err(Failure::GiveUp);
            }
            Err(err) => {
                tracing::warn!(%err, "discord send failed");
                return Err(Failure::Retry);
            }
        };

        // without the reference the decision can never be matched back, so treat it as failed
        // and let the next attempt re-send rather than orphan the message.
        if let Err(err) = self
            .remember_message(payment_id, channel_id, message.id)
            .await
        {
            tracing::error!(%err, %payment_id, "failed to persist discord message reference");
            return Err(Failure::Retry);
        }

        for emoji in [APPROVE_EMOJI, REJECT_EMOJI] {
            let reaction = RequestReactionType::Unicode { name: emoji };
            if let Err(err) = self
                .client
                .create_reaction(channel_id, message.id, &reaction)
                .await
            {
                // not retried: retrying would repost the whole message, and the listener
                // picks up a manually-added reaction anyway.
                tracing::warn!(%err, emoji, "failed to add reaction");
            }
        }

        Ok(())
    }

    async fn withdraw(&self, payment_id: Uuid) -> Result<(), Failure> {
        let reference = match self.find_message(payment_id).await {
            Ok(reference) => reference,
            Err(err) => {
                tracing::warn!(%err, "failed to look up discord message reference");
                return Err(Failure::Retry);
            }
        };
        // no reference means this payment never had a prompt - e.g. acked on creation, not
        // an error.
        let Some((channel_id, message_id)) = reference else {
            return Ok(());
        };

        if let Err(err) = self.client.delete_message(channel_id, message_id).await {
            let already_gone = matches!(
                err.kind(),
                ErrorType::Response { status, .. } if status.get() == 404
            );
            if !already_gone {
                tracing::warn!(%err, "failed to delete discord message");
                return Err(Failure::Retry);
            }
        }
        if let Err(err) = self.forget_message(payment_id).await {
            tracing::warn!(%err, "failed to delete discord message reference");
            return Err(Failure::Retry);
        }
        Ok(())
    }
}

#[async_trait]
impl StartupTask for DiscordConsumer {
    fn name(&self) -> &str {
        "discord-consumer"
    }

    async fn run(&self) {
        loop {
            let received = self.receiver.lock().await.recv().await;
            match received {
                Ok(message) => self.process(message).await,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "discord consumer lagged, some events were dropped")
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        tracing::warn!("event bus closed");
    }
}

impl DiscordConsumer {
    async fn process(&self, message: OutboxMessage) {
        let outcome = retrying(
            DELIVERY_ATTEMPTS,
            |failure| *failure == Failure::Retry,
            || self.handle(&message),
        )
        .await;

        match outcome {
            Ok(()) | Err(Failure::GiveUp) => {}
            Err(Failure::Retry) => {
                tracing::error!(
                    event_type = %message.event_type,
                    "discord delivery failed, retrying later"
                );
                tokio::time::sleep(self.nack_delay).await;
                self.bus.publish(message);
            }
        }
    }
}

fn gmail_link(message_id: &str) -> String {
    let id = message_id.trim_matches(|c| c == '<' || c == '>');
    let encoded: String = id
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("https://mail.google.com/mail/u/0/#search/rfc822msgid%3A{encoded}")
}

fn decision_from_emoji(emoji: &EmojiReactionType) -> Option<bool> {
    match emoji {
        EmojiReactionType::Unicode { name } if name == APPROVE_EMOJI => Some(true),
        EmojiReactionType::Unicode { name } if name == REJECT_EMOJI => Some(false),
        _ => None,
    }
}

/// Pushes reactions in as they happen; a reconcile job would be the slow catch-up for
/// anything missed while disconnected - deliberately out of scope for now.
pub struct DiscordReactionListener {
    token: String,
    pool: SqlitePool,
    payments: Arc<payment::Service>,
}

impl DiscordReactionListener {
    pub fn new(token: String, pool: SqlitePool, payments: Arc<payment::Service>) -> Self {
        Self {
            token,
            pool,
            payments,
        }
    }

    async fn payment_id(&self, message_id: Id<MessageMarker>) -> sqlx::Result<Option<Uuid>> {
        let row = sqlx::query(
            "SELECT confirmation_id FROM discord_confirmation_messages WHERE message_id = ?",
        )
        .bind(message_id.get() as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| {
            let id: String = row.get("confirmation_id");
            Uuid::parse_str(&id)
                .inspect_err(|_| tracing::warn!(id, "unparsable payment id in db"))
                .ok()
        }))
    }
}

#[async_trait]
impl StartupTask for DiscordReactionListener {
    fn name(&self) -> &str {
        "discord-reaction-listener"
    }

    async fn run(&self) {
        // the bot reacts to its own prompt to give the human something to click, so without
        // knowing its own id every prompt would instantly self-approve.
        let own_id = match Client::new(self.token.clone()).current_user().await {
            Ok(response) => match response.model().await {
                Ok(user) => user.id,
                Err(err) => {
                    tracing::error!(%err, "failed to decode discord current user");
                    return;
                }
            },
            Err(err) => {
                tracing::error!(%err, "failed to identify discord bot user, reactions will only be picked up by manual review");
                return;
            }
        };

        let mut shard = Shard::new(
            ShardId::ONE,
            self.token.clone(),
            Intents::GUILD_MESSAGE_REACTIONS | Intents::DIRECT_MESSAGE_REACTIONS,
        );

        while let Some(item) = shard.next_event(EventTypeFlags::REACTION_ADD).await {
            let event = match item {
                Ok(event) => event,
                Err(err) => {
                    tracing::warn!(%err, "discord gateway receive error");
                    continue;
                }
            };
            let GatewayEvent::ReactionAdd(reaction) = event else {
                continue;
            };
            if reaction.user_id == own_id {
                continue;
            }
            let Some(approved) = decision_from_emoji(&reaction.emoji) else {
                continue;
            };
            match self.payment_id(reaction.message_id).await {
                Ok(Some(payment_id)) => {
                    let _ = self.payments.ack(payment_id, approved).await;
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(%err, "failed to resolve reacted message"),
            }
        }

        tracing::warn!("discord gateway stream ended");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::Money;
    use crate::domain::money::some_money;
    use crate::domain::payment::some_standard_details;
    use crate::domain::payment::{Details, TaxForm};
    use crate::infra::event_bus::TokioEventBus;
    use crate::infra::persistence::connect;

    async fn consumer() -> DiscordConsumer {
        let pool = connect("sqlite::memory:").await.unwrap();
        consumer_with(pool, Arc::new(TokioEventBus::new()))
    }

    fn email_source() -> Source {
        Source::Email {
            message_id: "<CAx+1@mail.gmail.com>".into(),
        }
    }

    async fn stored(pool: &SqlitePool, source: Source) -> Payment {
        let (payment, _) = Payment::new(some_money(), some_standard_details(), source);
        payment_repo::create(pool, &payment).await.unwrap()
    }

    #[test]
    fn prompt_includes_title_and_amount() {
        let (payment, _event) = Payment::new(
            Money::from_parts("PLN", "99.99").unwrap(),
            some_standard_details(),
            Source::Manual,
        );

        assert_eq!(
            DiscordConsumer::prompt(&payment),
            "foo - someone: 99.99 PLN requires confirmation, click ✅ to confirm or ❎ to reject"
        );
    }

    #[test]
    fn prompt_links_the_email() {
        let (payment, _event) = Payment::new(
            Money::from_parts("PLN", "150.00").unwrap(),
            Details::tax(TaxForm::Ppl),
            email_source(),
        );

        assert_eq!(
            DiscordConsumer::prompt(&payment)
                .lines()
                .collect::<Vec<_>>(),
            vec![
                "PPE: 150.00 PLN requires confirmation, click ✅ to confirm or ❎ to reject",
                "Email: https://mail.google.com/mail/u/0/#search/rfc822msgid%3ACAx%2B1%40mail.gmail.com",
            ]
        );
    }

    #[tokio::test]
    async fn message_reference_round_trips_and_forgets() {
        let consumer = consumer().await;
        let payment_id = Uuid::new_v4();
        let channel_id = Id::<ChannelMarker>::new(111);
        let message_id = Id::<MessageMarker>::new(222);

        assert_eq!(consumer.find_message(payment_id).await.unwrap(), None);

        consumer
            .remember_message(payment_id, channel_id, message_id)
            .await
            .unwrap();
        assert_eq!(
            consumer.find_message(payment_id).await.unwrap(),
            Some((channel_id, message_id))
        );

        consumer.forget_message(payment_id).await.unwrap();
        assert_eq!(consumer.find_message(payment_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn handle_acked_with_no_stored_reference_is_a_no_op() {
        let consumer = consumer().await;
        let msg = OutboxMessage::from(Event::Acked { id: Uuid::new_v4() });

        assert_eq!(consumer.handle(&msg).await, Ok(()));
    }

    #[tokio::test]
    async fn handle_created_for_an_already_acked_payment_is_a_no_op() {
        let consumer = consumer().await;
        let payment = stored(&consumer.pool, Source::Manual).await;
        assert_ne!(payment.ack, Ack::Pending);

        let msg = OutboxMessage::from(Event::Created { id: payment.id });

        assert_eq!(consumer.handle(&msg).await, Ok(()));
    }

    #[tokio::test]
    async fn handle_redelivered_created_does_not_prompt_again() {
        let consumer = consumer().await;
        let payment = stored(&consumer.pool, email_source()).await;
        consumer
            .remember_message(payment.id, Id::new(111), Id::new(222))
            .await
            .unwrap();

        // no `[discord.channels]` configured, so reaching the send would be `Retry`.
        let msg = OutboxMessage::from(Event::Created { id: payment.id });

        assert_eq!(consumer.handle(&msg).await, Ok(()));
    }

    #[tokio::test]
    async fn handle_created_for_a_missing_payment_is_a_no_op() {
        let consumer = consumer().await;
        let msg = OutboxMessage::from(Event::Created { id: Uuid::new_v4() });

        assert_eq!(consumer.handle(&msg).await, Ok(()));
    }

    #[tokio::test]
    async fn handle_ignores_other_events() {
        let consumer = consumer().await;
        let finished = OutboxMessage::from(Event::Finished { id: Uuid::new_v4() });
        let ack_event = OutboxMessage {
            id: Uuid::new_v4(),
            event_type: "ACK_PENDING_CREATED".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            payload: b"not json".to_vec(),
        };

        assert_eq!(consumer.handle(&finished).await, Ok(()));
        assert_eq!(consumer.handle(&ack_event).await, Ok(()));
    }

    fn garbage() -> OutboxMessage {
        OutboxMessage {
            id: Uuid::new_v4(),
            event_type: "PAYMENT_CREATED".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            payload: b"not json".to_vec(),
        }
    }

    #[tokio::test]
    async fn handle_garbage_payload_gives_up() {
        let consumer = consumer().await;
        assert_eq!(consumer.handle(&garbage()).await, Err(Failure::GiveUp));
    }

    #[test]
    fn decision_from_emoji_ignores_unrelated_emoji() {
        let emoji = EmojiReactionType::Unicode {
            name: "🔥".to_string(),
        };
        assert_eq!(decision_from_emoji(&emoji), None);
    }

    #[test]
    fn decision_from_emoji_reads_approve_and_reject() {
        let approve = EmojiReactionType::Unicode {
            name: APPROVE_EMOJI.to_string(),
        };
        let reject = EmojiReactionType::Unicode {
            name: REJECT_EMOJI.to_string(),
        };
        assert_eq!(decision_from_emoji(&approve), Some(true));
        assert_eq!(decision_from_emoji(&reject), Some(false));
    }

    fn consumer_with(pool: sqlx::SqlitePool, bus: Arc<dyn EventBus>) -> DiscordConsumer {
        let receiver = bus.subscribe();
        DiscordConsumer {
            client: Client::new(String::new()),
            channels: HashMap::new(),
            pool,
            bus,
            receiver: Mutex::new(receiver),
            nack_delay: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn process_does_not_republish_a_delivery_it_can_handle() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let msg = OutboxMessage::from(Event::Acked { id: Uuid::new_v4() });
        let bus = Arc::new(TokioEventBus::new());
        let mut spy = bus.subscribe();
        let consumer = consumer_with(pool, bus);

        consumer.process(msg).await;

        let republished = tokio::time::timeout(Duration::from_millis(50), spy.recv()).await;
        assert!(republished.is_err(), "expected no republish");
    }

    #[tokio::test]
    async fn process_republishes_a_delivery_that_keeps_failing() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payment = stored(&pool, email_source()).await;
        assert_eq!(payment.ack, Ack::Pending);
        // no `[discord.channels]` entry configured, so `post_prompt` returns `Retry` without
        // ever touching the network.
        let msg = OutboxMessage::from(Event::Created { id: payment.id });
        let bus = Arc::new(TokioEventBus::new());
        let mut spy = bus.subscribe();
        let consumer = consumer_with(pool, bus);

        consumer.process(msg.clone()).await;

        let republished = tokio::time::timeout(Duration::from_millis(200), spy.recv())
            .await
            .expect("expected a republish")
            .unwrap();
        assert_eq!(republished.id, msg.id);
    }

    #[tokio::test]
    async fn process_does_not_republish_an_undecodable_payload() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let bus = Arc::new(TokioEventBus::new());
        let mut spy = bus.subscribe();
        let consumer = consumer_with(pool, bus);

        consumer.process(garbage()).await;

        let republished = tokio::time::timeout(Duration::from_millis(50), spy.recv()).await;
        assert!(republished.is_err(), "expected no republish");
    }

    #[tokio::test]
    async fn listener_resolves_a_message_back_to_its_payment() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let consumer = consumer_with(pool.clone(), Arc::new(TokioEventBus::new()));
        let payment_id = Uuid::new_v4();
        let message_id = Id::<MessageMarker>::new(222);
        consumer
            .remember_message(payment_id, Id::new(111), message_id)
            .await
            .unwrap();

        let listener = DiscordReactionListener::new(
            String::new(),
            pool.clone(),
            Arc::new(payment::Service::new(pool)),
        );

        assert_eq!(
            listener.payment_id(message_id).await.unwrap(),
            Some(payment_id)
        );
        assert_eq!(listener.payment_id(Id::new(999)).await.unwrap(), None);
    }
}
