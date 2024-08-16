use super::handle_request::{ChatGptReqBody, Parameters, SlackHistoryResponse};
use super::slack_message::SlackMessage;
use crate::constants::*;
use reqwest::{header, Client};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error as StdError;

#[derive(Debug, Clone)]
pub struct ApiClient {
    client: Client,
    slack_token: String,
    openai_token: String,
    channel: String,
}

impl ApiClient {
    pub fn new(params: &Parameters, channel: &str) -> Self {
        ApiClient {
            client: Client::new(),
            slack_token: params.slack_auth_token.clone(),
            openai_token: params.openai_secret_key.clone(),
            channel: channel.to_string(),
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
    ) -> Result<String, Box<dyn StdError>> {
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
        let res_json: Value = serde_json::from_str(&res_text)?;
        if res_json["ok"] != true {
            return Err(format!("Slack post error: {}", res_text).into());
        }
        Ok(res_json["ts"].as_str().unwrap().to_owned())
    }

    // slackのメッセージを更新する
    pub async fn update_message(&self, text: &str, ts: &str) -> Result<(), Box<dyn StdError>> {
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
        let res_json: Value = serde_json::from_str(&res_text)?;
        if res_json["ok"] != true {
            return Err(format!("Slack update error: {}", res_text).into());
        }
        Ok(())
    }

    // スレッド内のメッセージを取得する
    pub async fn get_replies(
        &self,
        thread_ts: &str,
        limit: &str,
    ) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
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
            return Err(format!("Error: {}", res.status()).into());
        }

        let body = res.text().await?;
        let json: SlackHistoryResponse = serde_json::from_str(&body)?;
        return Ok(json.messages);
    }

    // チャンネル内のメッセージを取得する
    pub async fn get_history(&self, limit: &str) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
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
            return Err(format!("Error: {}", res.status()).into());
        }

        let body = res.text().await?;
        let json: SlackHistoryResponse = serde_json::from_str(&body)?;
        return Ok(json.messages);
    }

    // ChatGPTにメッセージを投げて返答を取得する
    pub async fn get_chat_gpt_response(
        &self,
        request_body: ChatGptReqBody,
        ts: &str,
    ) -> Result<reqwest::Response, Box<dyn StdError>> {
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
                return Err("ChatGPT usage limit".into());
            }
            400 => {
                let body = res.text().await?;
                if body.contains("invalid_image_format") {
                    self.update_message(INVALID_IMAGE_FORMAT, ts).await?;
                } else {
                    self.update_message(ERROR_FROM_OPEN_AI_MESSAGE, ts).await?;
                }
                println!("Error from ChatGPT: {}", body);
                // println!("request body: {}", json!(request_body));
                return Err("error from chatgpt".into());
            }
            _ => {
                let body = res.text().await?;
                self.update_message(ERROR_FROM_OPEN_AI_MESSAGE, ts).await?;
                println!("Error from ChatGPT: {}", body);
                return Err("error from chatgpt".into());
            }
        }
    }
}
