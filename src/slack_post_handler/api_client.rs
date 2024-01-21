use super::handle_request::{ChatGptReqBody, Parameters};
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

    // // slackのメッセージに対する返信を取得する
    // pub async fn get_replies(
    //     &self,
    //     ts: &str,
    // ) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    //     let mut form = HashMap::new();
    //     form.insert("channel", &self.channel);
    //     form.insert("ts", ts);
    //     let res = self
    //         .client
    //         .post(SLACK_GET_REPLIES_URL)
    //         .headers(self.headers_for_slack())
    //         .form(&form)
    //         .send()
    //         .await?;
    //     let res_text = res.text().await?;
    //     let res_json: Value = serde_json::from_str(&res_text)?;
    //     if res_json["ok"] != true {
    //         return Err(format!("Slack get replies error: {}", res_text).into());
    //     }
    //     let messages: Vec<SlackMessage> = serde_json::from_value(res_json["messages"].clone())?;
    //     Ok(messages)
    // }

    // // slackのメッセージ履歴を取得する
    // pub async fn get_history(
    //     &self,
    //     latest: Option<&str>,
    //     oldest: Option<&str>,
    // ) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    //     let mut form = HashMap::new();
    //     form.insert("channel", &self.channel);
    //     if let Some(latest) = latest {
    //         form.insert("latest", latest);
    //     }
    //     if let Some(oldest) = oldest {
    //         form.insert("oldest", oldest);
    //     }
    //     let res = self
    //         .client
    //         .post(SLACK_GET_HISTORY_URL)
    //         .headers(self.headers_for_slack())
    //         .form(&form)
    //         .send()
    //         .await?;
    //     let res_text = res.text().await?;
    //     let res_json: Value = serde_json::from_str(&res_text)?;
    //     if res_json["ok"] != true {
    //         return Err(format!("Slack get history error: {}", res_text).into());
    //     }
    //     let messages: Vec<SlackMessage> = serde_json::from_value(res_json["messages"].clone())?;
    //     Ok(messages)
    // }

    // // ChatGPTにメッセージを投げて返答を取得する
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
            _ => {
                let body = res.text().await?;
                println!("Error from ChatGPT: {}", body);
                return Err("error from chatgpt".into());
            }
        }
    }
}
