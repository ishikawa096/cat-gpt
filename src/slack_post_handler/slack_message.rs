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

    // メンション文字列とコマンドを削除したメッセージ本文
    pub fn pure_text(&self) -> String {
        // メンション文字列
        let re = Regex::new(r"^<.+> ").unwrap();
        // past(数字)(過去のメッセージを参照するコマンド)
        let command_re = Regex::new(r"^past(\d+)").unwrap();
        let result = re.replace(&self.text, "").to_string();
        command_re.replace(&result, "").trim().to_string()
    }

    // past(数字)コマンドの数字を取得する
    pub fn get_limit(&self, default: i32, max_past: i32) -> i32 {
        let re: Regex = Regex::new(r"^past(\d+)").unwrap();
        let past = re
            .captures(&self.text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        let past_num = match past {
            Some(past) => match past.parse::<i32>() {
                Ok(num) => {
                    if num > max_past {
                        max_past
                    } else if num < 0 {
                        0
                    } else {
                        num
                    }
                }
                Err(_) => default,
            },
            None => default,
        };
        // 最新のメッセージの分を+1する
        past_num + 1
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
            content: self.pure_text(),
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
            // DM以外かつスレッド外の場合は自身のts(新規スレッドを作る)
            Some(self.ts.clone())
        }
    }

    pub fn reply_required(&self, bot_id: &str) -> bool {
        // typeがメッセージで、subtypeがなく、Bot自身のメッセージでない場合、処理を続行する
        self.type_name == "message" && self.subtype.is_none() && !self.is_from(bot_id)
    }
}

#[test]
fn test_pure_text() {
    let message = SlackMessage {
        text: "<@U01J9QZQZ9Z> <@U01YH89HJ2K> past10こんにちはpast3".to_string(),
        thread_ts: None,
        type_name: "message".to_string(),
        subtype: None,
        user: "U01J9QZQZ9Z".to_string(),
        channel: Some("D024BE91L".to_string()),
        ts: "1627777777.000000".to_string(),
        channel_type: None,
    };
    assert_eq!(message.pure_text(), "こんにちはpast3");
}

#[test]
fn test_get_limit() {
    let message = SlackMessage {
        text: "past10\nこんにちはpast0".to_string(),
        thread_ts: None,
        type_name: "message".to_string(),
        subtype: None,
        user: "U01J9QZQZ9Z".to_string(),
        channel: Some("D024BE91L".to_string()),
        ts: "1627777777.000000".to_string(),
        channel_type: None,
    };
    assert_eq!(message.get_limit(5, 10), 11);
}
