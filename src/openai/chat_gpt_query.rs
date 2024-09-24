use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::future::join_all;
use reqwest::{header::HeaderValue, Client};
use serde::Serialize;
use serde_derive::Deserialize;

use crate::constants::CHAT_GPT_SYSTEM_PROMPT;

use crate::slack_post_handler::slack_message::SlackMessage;

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Assistant,
    User,
    System,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatGptQuery {
    pub role: Role,
    pub content: ChatGptQueryContentEnum,
}

#[derive(Deserialize, Debug)]
pub enum ChatGptQueryContentEnum {
    QueryContent(Vec<QueryContent>),
    Text(String),
}

impl Serialize for ChatGptQueryContentEnum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ChatGptQueryContentEnum::Text(text) => serializer.serialize_str(text),
            ChatGptQueryContentEnum::QueryContent(objects) => objects.serialize(serializer),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QueryContent {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<ImageUrl>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ImageUrl {
    url: String,
}

impl ChatGptQuery {
    // システムプロンプトを生成
    pub fn new_system_prompt() -> Self {
        Self {
            role: Role::System,
            content: ChatGptQueryContentEnum::Text(CHAT_GPT_SYSTEM_PROMPT.to_string()),
        }
    }

    // SlackメッセージをChatGPTのクエリメッセージ形式に変換する
    pub async fn new_from_slack_messages(
        messages: Vec<SlackMessage>,
        bot_member_id: &str,
        slack_auth_token: &str,
    ) -> Vec<ChatGptQuery> {
        let chat_gpt_queries_futures = messages
            .into_iter()
            .map(|m| ChatGptQuery::new_from_slack_message(m, bot_member_id, slack_auth_token));

        join_all(chat_gpt_queries_futures)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect()
    }

    // SlackメッセージをChatGPTのクエリメッセージ形式に変換する
    async fn new_from_slack_message(
        message: SlackMessage,
        bot_id: &str,
        slack_auth_token: &str,
    ) -> Result<Self> {
        let role = if message.is_from(bot_id) {
            Role::Assistant
        } else {
            Role::User
        };

        let text = message.pure_text();
        let content = if message.files.is_some() {
            // ファイルがある場合はテキストと画像を組み合わせる
            let text_contents = vec![QueryContent {
                type_name: "text".into(),
                text: Some(text),
                image_url: None,
            }];

            let files = message.files.as_ref().unwrap();
            let file_contents_futures = files.iter().map(|f| async {
                let api_client = Client::new();
                let file = api_client
                    .get(f.url_private.clone())
                    .header(
                        "Authorization",
                        format!("Bearer {}", &slack_auth_token)
                            .parse::<HeaderValue>()
                            .unwrap(),
                    )
                    .send()
                    .await?;
                // fileをbase64エンコードする
                let file_base64 = STANDARD.encode(file.bytes().await?);
                // f"data:image/jpeg;base64,{file_base64}"の形式にする
                let image_url = format!("data:{};base64,{}", f.mimetype.clone(), file_base64);

                Ok::<QueryContent, reqwest::Error>(QueryContent {
                    type_name: "image_url".into(),
                    image_url: Some(ImageUrl { url: image_url }),
                    text: None,
                })
            });
            let file_contents: Vec<QueryContent> = join_all(file_contents_futures)
                .await
                .into_iter()
                .filter_map(Result::ok)
                .collect();

            let combined_content = text_contents
                .into_iter()
                .chain(file_contents.into_iter())
                .collect::<Vec<QueryContent>>();
            ChatGptQueryContentEnum::QueryContent(combined_content)
        } else {
            // ファイルがない場合はテキストのみ
            ChatGptQueryContentEnum::Text(text)
        };

        Ok(Self {
            role: role,
            content: content,
        })
    }
}
