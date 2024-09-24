use anyhow::Result;
use futures::future::join_all;
use serde::Serialize;
use serde_derive::Deserialize;

use crate::constants::CHAT_GPT_SYSTEM_PROMPT;

use crate::slack_post_handler::slack_message::SlackMessage;

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Assistant,
    User,
    // System,
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
    /**
     * preview版APIがシステムプロンプトに対応していないため、メッセージの文頭にシステムプロンプトを追加する
     */
    fn add_system_prompt(&mut self) {
        let system_prompt = ChatGptQueryContentEnum::Text(CHAT_GPT_SYSTEM_PROMPT.to_string());
        let new_text = format!("{:?}{:?}", system_prompt, self.content);
        self.content = ChatGptQueryContentEnum::Text(new_text);
    }

    // SlackメッセージをChatGPTのクエリメッセージ形式に変換する
    pub async fn new_from_slack_messages(
        messages: Vec<SlackMessage>,
        bot_member_id: &str,
    ) -> Vec<ChatGptQuery> {
        let chat_gpt_queries_futures = messages
            .into_iter()
            .map(|m| ChatGptQuery::new_from_slack_message(m, bot_member_id));

        let mut queries: Vec<ChatGptQuery> = join_all(chat_gpt_queries_futures)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect();

        // 最後のメッセージの文頭にシステムプロンプトを追加
        if let Some(last_query) = queries.last_mut() {
            last_query.add_system_prompt();
        };

        queries
    }

    /**
     * SlackメッセージをChatGPTのクエリメッセージ形式に変換する
     */
    async fn new_from_slack_message(message: SlackMessage, bot_id: &str) -> Result<Self> {
        let role = if message.is_from(bot_id) {
            Role::Assistant
        } else {
            Role::User
        };

        let text = message.pure_text();
        // preview版APIが画像に対応していないため、テキストのみ
        let content = ChatGptQueryContentEnum::Text(text);

        Ok(Self {
            role: role,
            content: content,
        })
    }
}
