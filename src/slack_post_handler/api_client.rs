use super::handle_request::{ChatGptReqBody, Parameters, SlackHistoryResponse};
use super::slack_message::SlackMessage;
use crate::constants::*;
use anyhow::Result;
use reqwest::StatusCode;
use reqwest::{header, Client};
use serde_json::{json, Value};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ApiClient {
    client: Client,
    slack_token: String,
    openai_token: String,
    channel: String,
}

#[derive(Error, Debug)]
pub enum ApiClientError {
    #[error("Request failed with status: {0}, at {1}")]
    StatusError(StatusCode, &'static str),
    #[error("Failed to parse: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("Slack post error: {0}")]
    SlackPostError(String),
    #[error("Slack update error: {0}")]
    SlackUpdateError(String),
    #[error("OpenAI API usage limit.")]
    OpenaiUsageLimit(),
    #[error("OpenAI API error: {0}")]
    OpenaiError(String),
}

impl ApiClient {
    pub fn new(params: &Parameters, channel: &str) -> Self {
        ApiClient {
            client: Client::new(),
            slack_token: params.slack_auth_token.clone(),
            openai_token: params.openai_secret_key.clone(),
            channel: channel.into(),
        }
    }

    // slack headers
    fn headers_for_slack(&self) -> header::HeaderMap {
        let mut headers = header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.slack_token).parse().unwrap(),
        );
        headers
    }

    fn headers_for_openai(&self) -> header::HeaderMap {
        let mut headers = header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.openai_token).parse().unwrap(),
        );
        headers
    }

    // slackにメッセージを投稿する
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<String> {
        let mut form = HashMap::new();
        form.insert("channel", channel);
        form.insert("text", text);
        if let Some(thread_ts) = thread_ts {
            form.insert("thread_ts", thread_ts);
        }
        let res = self
            .client
            .post(SLACK_POST_URL)
            .headers(self.headers_for_slack())
            .form(&form)
            .send()
            .await?;
        let res_text = res.text().await?;
        let res_json: Value =
            serde_json::from_str(&res_text).map_err(ApiClientError::ParseError)?;
        if res_json["ok"] != true {
            return Err(ApiClientError::SlackPostError(res_text).into());
        }
        Ok(res_json["ts"].as_str().unwrap().to_owned())
    }

    // slackのメッセージを更新する
    pub async fn update_message(&self, text: &str, ts: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let text_string = text.to_string();
        let ts_string = ts.to_string();
        let mut form = HashMap::new();
        form.insert("channel", &self.channel);
        form.insert("text", &text_string);
        form.insert("ts", &ts_string);
        // TODO: レート制限にかかった場合に対応する
        let res = self
            .client
            .post(SLACK_UPDATE_URL)
            .headers(self.headers_for_slack())
            .form(&form)
            .send()
            .await?;
        let res_text = res.text().await?;
        let res_json: Value =
            serde_json::from_str(&res_text).map_err(ApiClientError::ParseError)?;
        if res_json["ok"] != true {
            return Err(ApiClientError::SlackUpdateError(res_text).into());
        }
        Ok(())
    }

    // スレッド内のメッセージを取得する
    pub async fn get_replies(&self, thread_ts: &str, limit: &str) -> Result<Vec<SlackMessage>> {
        let query = &[
            ("limit", limit),
            ("channel", self.channel.as_str()),
            ("ts", thread_ts),
        ];

        let client = reqwest::Client::new();
        let res = client
            .get(SLACK_GET_REPLIES_URL)
            .headers(self.headers_for_slack())
            .query(query)
            .send()
            .await?;

        // エラーハンドリング
        if !res.status().is_success() {
            return Err(ApiClientError::StatusError(res.status(), "get_replies").into());
        }

        let body = res.text().await?;
        let json: SlackHistoryResponse =
            serde_json::from_str(&body).map_err(ApiClientError::ParseError)?;
        return Ok(json.messages);
    }

    // チャンネル内のメッセージを取得する
    pub async fn get_history(&self, limit: &str) -> Result<Vec<SlackMessage>> {
        let query = &[("limit", limit), ("channel", self.channel.as_str())];
        let res = self
            .client
            .get(SLACK_GET_HISTORY_URL)
            .headers(self.headers_for_slack())
            .query(query)
            .send()
            .await?;

        // エラーハンドリング
        if !res.status().is_success() {
            return Err(ApiClientError::StatusError(res.status(), "get_history").into());
        }

        let body = res.text().await?;
        let json: SlackHistoryResponse =
            serde_json::from_str(&body).map_err(ApiClientError::ParseError)?;
        return Ok(json.messages);
    }

    // ChatGPTにメッセージを投げて返答を取得する
    pub async fn get_chat_gpt_response(
        &self,
        request_body: ChatGptReqBody,
        ts: &str,
    ) -> Result<reqwest::Response> {
        let res = self
            .client
            .post(CHAT_GPT_POST_URL)
            .headers(self.headers_for_openai())
            .json(&request_body)
            .send()
            .await?;

        match res.status().as_u16() {
            200 => Ok(res),
            429 => {
                self.update_message(USAGE_LIMIT_MESSAGE, ts).await?;
                Err(ApiClientError::OpenaiUsageLimit().into())
            }
            400 => {
                let body = res.text().await?;
                let error_message = if body.contains("invalid_image_format") {
                    INVALID_IMAGE_FORMAT
                } else {
                    ERROR_FROM_OPEN_AI_MESSAGE
                };
                self.update_message(error_message, ts).await?;
                #[cfg(debug_assertions)]
                {
                    println!("request body: {}", json!(request_body));
                }
                Err(ApiClientError::OpenaiError(body).into())
            }
            _ => {
                self.update_message(ERROR_FROM_OPEN_AI_MESSAGE, ts).await?;
                #[cfg(debug_assertions)]
                {
                    println!("request body: {}", json!(request_body));
                }
                println!("request body: {}", json!(request_body));
                let body = res.text().await?;
                println!("res body: {}", json!(body));
                Err(ApiClientError::OpenaiError(body).into())
            }
        }
    }
}
