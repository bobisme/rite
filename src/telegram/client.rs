use anyhow::{Context, Result, bail};
use reqwest::multipart;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::path::Path;
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

    /// Download a file from Telegram by file_id.
    ///
    /// 1. Calls `getFile` to get the temporary file_path
    /// 2. Downloads the bytes from Telegram's file server
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>> {
        // Step 1: Get file path from Telegram
        let url = self.api_url("getFile");
        let params = [("file_id", file_id.to_string())];

        let response = self
            .http
            .post(&url)
            .form(&params)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| "Telegram getFile request failed")?;

        let file_info: TelegramFile = parse_response(response).await?;
        let file_path = file_info
            .file_path
            .ok_or_else(|| anyhow::anyhow!("Telegram getFile response missing file_path"))?;

        // Step 2: Download the file bytes
        let download_url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );

        let response = self
            .http
            .get(&download_url)
            .timeout(Duration::from_secs(60)) // Longer timeout for file downloads
            .send()
            .await
            .with_context(|| "Telegram file download request failed")?;

        if !response.status().is_success() {
            bail!(
                "Telegram file download failed with HTTP {}",
                response.status()
            );
        }

        let bytes = response
            .bytes()
            .await
            .with_context(|| "Failed to read Telegram file bytes")?;

        Ok(bytes.to_vec())
    }

    /// Send a photo to a Telegram chat via multipart upload.
    pub async fn send_photo(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        self.send_media("sendPhoto", "photo", chat_id, thread_id, file_path, caption)
            .await
    }

    /// Send a document to a Telegram chat via multipart upload.
    pub async fn send_document(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        self.send_media(
            "sendDocument",
            "document",
            chat_id,
            thread_id,
            file_path,
            caption,
        )
        .await
    }

    /// Send a video to a Telegram chat via multipart upload.
    pub async fn send_video(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        self.send_media("sendVideo", "video", chat_id, thread_id, file_path, caption)
            .await
    }

    /// Send audio to a Telegram chat via multipart upload.
    pub async fn send_audio(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        self.send_media("sendAudio", "audio", chat_id, thread_id, file_path, caption)
            .await
    }

    /// Generic media sender using multipart form upload.
    async fn send_media(
        &self,
        method: &str,
        field_name: &str,
        chat_id: i64,
        thread_id: Option<i64>,
        file_path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        let url = self.api_url(method);

        let file_bytes = tokio::fs::read(file_path)
            .await
            .with_context(|| format!("Failed to read file: {}", file_path.display()))?;

        let file_name = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();

        let file_part = multipart::Part::bytes(file_bytes).file_name(file_name);

        let mut form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(field_name.to_string(), file_part);

        if let Some(thread_id) = thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }

        if let Some(caption) = caption {
            form = form.text("caption", caption.to_string());
        }

        let response = self
            .http
            .post(&url)
            .multipart(form)
            .timeout(Duration::from_secs(60)) // Longer timeout for uploads
            .send()
            .await
            .with_context(|| format!("Telegram {} request failed", method))?;

        let _: serde_json::Value = parse_response(response).await?;
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
    /// Caption for media messages (photos, documents, etc.)
    pub caption: Option<String>,
    pub chat: Chat,
    pub from: Option<User>,
    /// Photo sizes (Telegram sends multiple resolutions)
    #[serde(default)]
    pub photo: Vec<PhotoSize>,
    /// Document attachment
    pub document: Option<Document>,
    /// Audio attachment
    pub audio: Option<Audio>,
    /// Video attachment
    pub video: Option<Video>,
    /// Voice message
    pub voice: Option<Voice>,
    /// Animation (GIF)
    pub animation: Option<Animation>,
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

/// Telegram photo size (one resolution variant).
#[derive(Debug, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<i64>,
}

/// Telegram document.
#[derive(Debug, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram audio.
#[derive(Debug, Deserialize)]
pub struct Audio {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram video.
#[derive(Debug, Deserialize)]
pub struct Video {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram voice message.
#[derive(Debug, Deserialize)]
pub struct Voice {
    pub file_id: String,
    pub file_unique_id: String,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram animation (GIF).
#[derive(Debug, Deserialize)]
pub struct Animation {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Response from getFile API call.
#[derive(Debug, Deserialize)]
struct TelegramFile {
    #[allow(dead_code)]
    file_id: String,
    #[allow(dead_code)]
    file_unique_id: String,
    file_path: Option<String>,
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
