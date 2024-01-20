use super::handle_request::{ChatGptQuery, Role};
use regex::Regex;
use serde_derive::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct SlackMessage {
    pub text: String,
    pub thread_ts: Option<String>,

    #[serde(rename = "type")]
    pub type_name: String,
    pub subtype: Option<String>,
    pub user: String,
    pub channel: Option<String>,
    pub ts: String,
    pub channel_type: Option<String>,
}

impl SlackMessage {
    // 指定したユーザーへのメンションかどうか
    pub fn is_mention_to(&self, user_id: &str) -> bool {
        self.text.contains(&user_id)
    }

    // bot以外へのメンションかどうか
    pub fn is_mention_to_other(&self, bot_id: &str) -> bool {
        !self.is_mention_to(bot_id) && self.text.contains("<@")
    }

    // スレッド内のメッセージかどうか
    pub fn is_in_thread(&self) -> bool {
        self.thread_ts.is_some()
    }

    // DMかどうか
    pub fn is_direct_message(&self) -> bool {
        self.channel_type == Some("im".to_string())
    }

    // メッセージの送信者が指定したユーザーかどうか
    pub fn is_from(&self, user_id: &str) -> bool {
        self.user == user_id
    }

    // メンション文字列を削除したメッセージ本文
    pub fn text_without_mention_string(&self) -> String {
        let re = Regex::new(r"^<.+> ").unwrap();
        let result = re.replace(&self.text, "");
        result.into_owned()
    }

    // SlackメッセージをChatGPTのクエリメッセージ形式に変換する
    pub fn to_chat_gpt_query(&self, bot_id: &str) -> ChatGptQuery {
        let role = if self.is_from(bot_id) {
            Role::Assistant
        } else {
            Role::User
        };
        ChatGptQuery {
            role: role,
            content: self.text_without_mention_string(),
        }
    }

    // 新規メッセージのthread_tsを決定する
    pub fn new_message_thread_ts(&self) -> Option<String> {
        if self.is_in_thread() {
            // スレッド内の場合はスレッドに返信する
            self.thread_ts.clone()
        } else if self.is_direct_message() {
            // DMかつスレッド外の場合はリプライにしない = thread_ts無し
            None
        } else {
            // DM以外かつスレッド外の場合はスレッドを作る
            Some(self.ts.clone())
        }
    }

    pub fn reply_required(&self, bot_id: &str) -> bool {
        // typeがメッセージで、subtypeがなく、Bot自身のメッセージでない場合、処理を続行する
        self.type_name == "message" && self.subtype.is_none() && !self.is_from(bot_id)
    }
}
