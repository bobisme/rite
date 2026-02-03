use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Async Telegram client for the Bot API.
#[derive(Clone)]
pub struct TelegramClient {
    token: String,
    http: reqwest::Client,
}

const REQUEST_TIMEOUT_SECS: u64 = 10;

impl TelegramClient {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .with_context(|| "Failed to build Telegram HTTP client")?;

        Ok(Self {
            token: token.into(),
            http,
        })
    }

    pub async fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<Update>> {
        let url = self.api_url("getUpdates");
        let mut params = vec![("timeout", timeout.to_string())];
        if let Some(offset) = offset {
            params.push(("offset", offset.to_string()));
        }

        let request_timeout = Duration::from_secs(timeout.saturating_add(5));

        let response = self
            .http
            .post(&url)
            .form(&params)
            .timeout(request_timeout)
            .send()
            .await
            .with_context(|| "Telegram getUpdates request failed")?;

        parse_response(response).await
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Result<()> {
        let url = self.api_url("sendMessage");
        let mut params = vec![("chat_id", chat_id.to_string()), ("text", text.to_string())];
        if let Some(thread_id) = thread_id {
            params.push(("message_thread_id", thread_id.to_string()));
        }

        let response = self
            .http
            .post(&url)
            .form(&params)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| "Telegram sendMessage request failed")?;

        let _: serde_json::Value = parse_response(response).await?;
        Ok(())
    }

    pub async fn create_forum_topic(&self, chat_id: i64, name: &str) -> Result<i64> {
        let url = self.api_url("createForumTopic");
        let params = [("chat_id", chat_id.to_string()), ("name", name.to_string())];

        let response = self
            .http
            .post(&url)
            .form(&params)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| "Telegram createForumTopic request failed")?;

        let topic: ForumTopic = parse_response(response).await?;
        Ok(topic.message_thread_id)
    }

    pub async fn edit_forum_topic(&self, chat_id: i64, thread_id: i64, name: &str) -> Result<()> {
        let url = self.api_url("editForumTopic");
        let params = [
            ("chat_id", chat_id.to_string()),
            ("message_thread_id", thread_id.to_string()),
            ("name", name.to_string()),
        ];

        let response = self
            .http
            .post(&url)
            .form(&params)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| "Telegram editForumTopic request failed")?;

        let ok: bool = parse_response(response).await?;
        if !ok {
            bail!("Telegram editForumTopic returned false");
        }
        Ok(())
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.token, method)
    }
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub message_thread_id: Option<i64>,
    pub text: Option<String>,
    pub chat: Chat,
    pub from: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ForumTopic {
    message_thread_id: i64,
}

async fn parse_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| "Failed to read Telegram response body")?;

    let parsed: TelegramResponse<T> = serde_json::from_str(&body).with_context(|| {
        format!(
            "Failed to parse Telegram response (HTTP {}): {}",
            status, body
        )
    })?;

    if !parsed.ok {
        let desc = parsed
            .description
            .unwrap_or_else(|| "Unknown error".to_string());
        let code = parsed.error_code.unwrap_or(0);
        bail!("Telegram API error {} (HTTP {}): {}", code, status, desc);
    }

    parsed
        .result
        .ok_or_else(|| anyhow::anyhow!("Telegram API response missing result"))
}
