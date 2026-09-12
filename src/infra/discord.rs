use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::Client;
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::channel::message::{EmojiReactionType, Reaction};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker},
};
use uuid::Uuid;

use crate::domain::confirmation::{
    Confirmation, ConfirmationService, ConfirmationSubject, Notifier,
};
use crate::domain::payment::PaymentRepository;
use crate::domain::startup_task::StartupTask;

/// Discord's own max for this endpoint. The catch-up poll only sees a confirmation whose
/// message is still inside this window; the gateway listener covers everything else.
const MESSAGE_FETCH_LIMIT: u16 = 100;

const APPROVE_EMOJI: &str = "✅";
const REJECT_EMOJI: &str = "❎";

pub struct DiscordNotifier {
    client: Client,
    channels: HashMap<String, String>,
    payments: Arc<dyn PaymentRepository>,
    pool: SqlitePool,
}

impl DiscordNotifier {
    pub fn new(
        token: String,
        channels: HashMap<String, String>,
        payments: Arc<dyn PaymentRepository>,
        pool: SqlitePool,
    ) -> Self {
        Self {
            client: Client::new(token),
            channels,
            payments,
            pool,
        }
    }

    fn channel_id(&self, channel_key: &str) -> Option<Id<ChannelMarker>> {
        let raw = self.channels.get(channel_key)?;
        raw.parse::<u64>()
            .inspect_err(|_| tracing::warn!(channel_key, raw, "invalid discord channel id"))
            .ok()
            .map(Id::new)
    }

    async fn confirmation_prompt(&self, confirmation: &Confirmation) -> String {
        let subject = match confirmation.subject {
            ConfirmationSubject::Payment(payment_id) => {
                match self.payments.find_by_id(payment_id).await {
                    Ok(Some(payment)) => format!("payment of {}", payment.total),
                    Ok(None) => "payment".to_string(),
                    Err(err) => {
                        tracing::error!(%err, %payment_id, "failed to look up payment");
                        "payment".to_string()
                    }
                }
            }
        };
        format!(
            "PLACEHOLDER_TITLE {subject} requires confirmation, click {APPROVE_EMOJI} to confirm or {REJECT_EMOJI} to reject"
        )
    }

    async fn remember_message(
        &self,
        confirmation_id: Uuid,
        channel_id: Id<ChannelMarker>,
        message_id: Id<MessageMarker>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO discord_confirmation_messages
             (confirmation_id, channel_id, message_id) VALUES (?, ?, ?)",
        )
        .bind(confirmation_id.to_string())
        .bind(channel_id.get() as i64)
        .bind(message_id.get() as i64)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_message(
        &self,
        confirmation_id: Uuid,
    ) -> sqlx::Result<Option<(Id<ChannelMarker>, Id<MessageMarker>)>> {
        Ok(sqlx::query(
            "SELECT channel_id, message_id FROM discord_confirmation_messages
             WHERE confirmation_id = ?",
        )
        .bind(confirmation_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(|row| {
            let channel_id: i64 = row.get("channel_id");
            let message_id: i64 = row.get("message_id");
            (Id::new(channel_id as u64), Id::new(message_id as u64))
        }))
    }

    async fn forget_message(&self, confirmation_id: Uuid) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM discord_confirmation_messages WHERE confirmation_id = ?")
            .bind(confirmation_id.to_string())
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_messages(
        &self,
        confirmation_ids: &[Uuid],
    ) -> sqlx::Result<HashMap<Uuid, (Id<ChannelMarker>, Id<MessageMarker>)>> {
        if confirmation_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; confirmation_ids.len()].join(",");
        let sql = format!(
            "SELECT confirmation_id, channel_id, message_id FROM discord_confirmation_messages
             WHERE confirmation_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql);
        for id in confirmation_ids {
            query = query.bind(id.to_string());
        }
        Ok(query
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .filter_map(|row| {
                let confirmation_id: String = row.get("confirmation_id");
                let channel_id: i64 = row.get("channel_id");
                let message_id: i64 = row.get("message_id");
                Uuid::parse_str(&confirmation_id)
                    .inspect_err(|_| {
                        tracing::warn!(confirmation_id, "unparsable confirmation id in db")
                    })
                    .ok()
                    .map(|id| (id, (Id::new(channel_id as u64), Id::new(message_id as u64))))
            })
            .collect())
    }

    async fn recent_decisions(
        &self,
        channel_id: Id<ChannelMarker>,
    ) -> HashMap<Id<MessageMarker>, bool> {
        let messages = match self
            .client
            .channel_messages(channel_id)
            .limit(MESSAGE_FETCH_LIMIT)
            .await
        {
            Ok(response) => match response.model().await {
                Ok(messages) => messages,
                Err(err) => {
                    tracing::warn!(%err, "failed to decode discord channel messages response");
                    return HashMap::new();
                }
            },
            Err(err) => {
                tracing::warn!(%err, "failed to fetch discord channel messages");
                return HashMap::new();
            }
        };

        messages
            .into_iter()
            .filter_map(|message| {
                decision_from_reactions(&message.reactions).map(|d| (message.id, d))
            })
            .collect()
    }
}

fn decision_from_emoji(emoji: &EmojiReactionType) -> Option<bool> {
    match emoji {
        EmojiReactionType::Unicode { name } if name == APPROVE_EMOJI => Some(true),
        EmojiReactionType::Unicode { name } if name == REJECT_EMOJI => Some(false),
        _ => None,
    }
}

/// The bot's own auto-added reaction is always one of the count, so only `count > 1` means a
/// human reacted too. A message carrying both reactions is not an approval.
fn decision_from_reactions(reactions: &[Reaction]) -> Option<bool> {
    let decisions: Vec<bool> = reactions
        .iter()
        .filter(|reaction| reaction.count > 1)
        .filter_map(|reaction| decision_from_emoji(&reaction.emoji))
        .collect();
    (!decisions.is_empty()).then(|| decisions.iter().all(|approved| *approved))
}

#[async_trait]
impl Notifier for DiscordNotifier {
    async fn request(&self, confirmation: &Confirmation) -> bool {
        let Some(channel_id) = self.channel_id("confirmations") else {
            tracing::warn!("no discord channel configured, dropping message");
            return false;
        };

        let content = self.confirmation_prompt(confirmation).await;
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
                    return false;
                }
            },
            Err(err) => {
                tracing::warn!(%err, "discord send failed");
                return false;
            }
        };

        // without the reference the decision can never be matched back, so treat it as
        // undelivered and let the next reconcile re-send rather than orphan the message.
        if let Err(err) = self
            .remember_message(confirmation.id, channel_id, message.id)
            .await
        {
            tracing::error!(%err, confirmation_id = %confirmation.id, "failed to persist discord message reference");
            return false;
        }

        for emoji in [APPROVE_EMOJI, REJECT_EMOJI] {
            let reaction = RequestReactionType::Unicode { name: emoji };
            if let Err(err) = self
                .client
                .create_reaction(channel_id, message.id, &reaction)
                .await
            {
                tracing::warn!(%err, emoji, "failed to add reaction");
            }
        }

        true
    }

    async fn withdraw(&self, confirmation: &Confirmation) {
        let reference = match self.find_message(confirmation.id).await {
            Ok(reference) => reference,
            Err(err) => {
                tracing::warn!(%err, "failed to look up discord message reference");
                return;
            }
        };
        let Some((channel_id, message_id)) = reference else {
            return;
        };
        if let Err(err) = self.client.delete_message(channel_id, message_id).await {
            tracing::warn!(%err, "failed to delete discord message");
        }
        if let Err(err) = self.forget_message(confirmation.id).await {
            tracing::warn!(%err, "failed to delete discord message reference");
        }
    }

    async fn poll_decisions(&self, confirmations: &[Confirmation]) -> HashMap<Uuid, bool> {
        let ids: Vec<Uuid> = confirmations.iter().map(|c| c.id).collect();
        let references = match self.find_messages(&ids).await {
            Ok(references) => references,
            Err(err) => {
                tracing::warn!(%err, "failed to look up discord message references");
                return HashMap::new();
            }
        };
        if references.is_empty() {
            return HashMap::new();
        }

        let channels: HashSet<Id<ChannelMarker>> = references
            .values()
            .map(|(channel_id, _)| *channel_id)
            .collect();
        let mut decisions_by_message = HashMap::new();
        for channel_id in channels {
            decisions_by_message.extend(self.recent_decisions(channel_id).await);
        }

        references
            .into_iter()
            .filter_map(|(confirmation_id, (_, message_id))| {
                decisions_by_message
                    .get(&message_id)
                    .map(|&approved| (confirmation_id, approved))
            })
            .collect()
    }
}

/// Pushes reactions in as they happen; `ConfirmationReconcileJob` is the slow catch-up for
/// anything missed while disconnected.
pub struct DiscordReactionListener {
    token: String,
    pool: SqlitePool,
    confirmations: Arc<ConfirmationService>,
}

impl DiscordReactionListener {
    pub fn new(token: String, pool: SqlitePool, confirmations: Arc<ConfirmationService>) -> Self {
        Self {
            token,
            pool,
            confirmations,
        }
    }

    async fn confirmation_id(&self, message_id: Id<MessageMarker>) -> sqlx::Result<Option<Uuid>> {
        let row = sqlx::query(
            "SELECT confirmation_id FROM discord_confirmation_messages WHERE message_id = ?",
        )
        .bind(message_id.get() as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| {
            let id: String = row.get("confirmation_id");
            Uuid::parse_str(&id)
                .inspect_err(|_| tracing::warn!(id, "unparsable confirmation id in db"))
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
                tracing::error!(%err, "failed to identify discord bot user, reactions will only be picked up by the reconcile job");
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
            let Event::ReactionAdd(reaction) = event else {
                continue;
            };
            if reaction.user_id == own_id {
                continue;
            }
            let Some(approved) = decision_from_emoji(&reaction.emoji) else {
                continue;
            };
            match self.confirmation_id(reaction.message_id).await {
                Ok(Some(confirmation_id)) => {
                    let _ = self.confirmations.decide(confirmation_id, approved).await;
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
    use crate::domain::confirmation::CreateConfirmationCommand;
    use crate::domain::money::Money;
    use crate::domain::payment::{Payment, PaymentSource};
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;

    fn some_confirmation(payment_id: Uuid) -> Confirmation {
        Confirmation::new(CreateConfirmationCommand {
            subject: ConfirmationSubject::Payment(payment_id),
            automatic_confirmation: false,
        })
    }

    async fn notifier() -> DiscordNotifier {
        let pool = connect("sqlite::memory:").await.unwrap();
        DiscordNotifier::new(
            String::new(),
            HashMap::new(),
            Arc::new(SqlitePaymentRepository::new(pool.clone())),
            pool,
        )
    }

    #[tokio::test]
    async fn confirmation_prompt_includes_amount_and_currency() {
        let notifier = notifier().await;
        let payment = Payment::new(
            Money::from_parts("PLN", "99.99").unwrap(),
            PaymentSource::Manual,
        );
        notifier.payments.create(payment.clone()).await.unwrap();

        let confirmation = some_confirmation(payment.id);
        let prompt = notifier.confirmation_prompt(&confirmation).await;

        assert_eq!(
            prompt,
            "PLACEHOLDER_TITLE payment of 99.99 PLN requires confirmation, click ✅ to confirm or ❎ to reject"
        );
    }

    #[tokio::test]
    async fn confirmation_prompt_falls_back_when_payment_missing() {
        let notifier = notifier().await;
        let confirmation = some_confirmation(Uuid::new_v4());
        let prompt = notifier.confirmation_prompt(&confirmation).await;

        assert_eq!(
            prompt,
            "PLACEHOLDER_TITLE payment requires confirmation, click ✅ to confirm or ❎ to reject"
        );
    }

    #[tokio::test]
    async fn message_reference_round_trips_and_forgets() {
        let notifier = notifier().await;
        let confirmation_id = Uuid::new_v4();
        let channel_id = Id::<ChannelMarker>::new(111);
        let message_id = Id::<MessageMarker>::new(222);

        assert_eq!(notifier.find_message(confirmation_id).await.unwrap(), None);

        notifier
            .remember_message(confirmation_id, channel_id, message_id)
            .await
            .unwrap();
        assert_eq!(
            notifier.find_message(confirmation_id).await.unwrap(),
            Some((channel_id, message_id))
        );

        notifier.forget_message(confirmation_id).await.unwrap();
        assert_eq!(notifier.find_message(confirmation_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn poll_decisions_returns_empty_when_no_message_is_known() {
        let notifier = notifier().await;
        let confirmation = some_confirmation(Uuid::new_v4());

        assert_eq!(
            notifier.poll_decisions(&[confirmation]).await,
            HashMap::new()
        );
    }

    #[tokio::test]
    async fn find_messages_batches_the_lookup_and_skips_unknown_ids() {
        let notifier = notifier().await;
        let known = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        let channel_id = Id::<ChannelMarker>::new(111);
        let message_id = Id::<MessageMarker>::new(222);
        notifier
            .remember_message(known, channel_id, message_id)
            .await
            .unwrap();

        let result = notifier.find_messages(&[known, unknown]).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(&known), Some(&(channel_id, message_id)));
    }

    #[tokio::test]
    async fn listener_resolves_a_message_back_to_its_confirmation() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let notifier = DiscordNotifier::new(
            String::new(),
            HashMap::new(),
            Arc::new(SqlitePaymentRepository::new(pool.clone())),
            pool.clone(),
        );
        let confirmation_id = Uuid::new_v4();
        let message_id = Id::<MessageMarker>::new(222);
        notifier
            .remember_message(confirmation_id, Id::new(111), message_id)
            .await
            .unwrap();

        let listener = DiscordReactionListener::new(
            String::new(),
            pool.clone(),
            Arc::new(ConfirmationService::new(
                Arc::new(
                    crate::infra::persistence::sqlx_confirmation_repository::SqliteConfirmationRepository::new(pool),
                ),
                Arc::new(crate::infra::notifier::LoggingNotifier),
            )),
        );

        assert_eq!(
            listener.confirmation_id(message_id).await.unwrap(),
            Some(confirmation_id)
        );
        assert_eq!(listener.confirmation_id(Id::new(999)).await.unwrap(), None);
    }

    fn reaction(emoji: &str, count: u64) -> Reaction {
        Reaction {
            burst_colors: Vec::new(),
            count,
            count_details: twilight_model::channel::message::ReactionCountDetails {
                burst: 0,
                normal: count,
            },
            emoji: EmojiReactionType::Unicode {
                name: emoji.to_string(),
            },
            me: true,
            me_burst: false,
        }
    }

    #[test]
    fn decision_from_reactions_ignores_the_bots_own_auto_added_reaction() {
        let reactions = [reaction(APPROVE_EMOJI, 1), reaction(REJECT_EMOJI, 1)];
        assert_eq!(decision_from_reactions(&reactions), None);
    }

    #[test]
    fn decision_from_reactions_detects_a_human_approval() {
        let reactions = [reaction(APPROVE_EMOJI, 2), reaction(REJECT_EMOJI, 1)];
        assert_eq!(decision_from_reactions(&reactions), Some(true));
    }

    #[test]
    fn decision_from_reactions_detects_a_human_rejection() {
        let reactions = [reaction(APPROVE_EMOJI, 1), reaction(REJECT_EMOJI, 2)];
        assert_eq!(decision_from_reactions(&reactions), Some(false));
    }

    #[test]
    fn decision_from_reactions_does_not_approve_when_both_were_clicked() {
        let reactions = [reaction(APPROVE_EMOJI, 2), reaction(REJECT_EMOJI, 2)];
        assert_eq!(decision_from_reactions(&reactions), Some(false));
    }

    #[test]
    fn decision_from_emoji_ignores_unrelated_emoji() {
        let emoji = EmojiReactionType::Unicode {
            name: "🔥".to_string(),
        };
        assert_eq!(decision_from_emoji(&emoji), None);
    }
}
