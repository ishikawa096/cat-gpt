use std::fmt;

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
    pub files: Option<Vec<SharedFile>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SharedFile {
    pub filetype: String,
    pub mimetype: String,
    pub url_private: String,
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

    // o1-preview指定かどうか
    pub fn is_o1_preview(&self) -> bool {
        let o1_preview_regex = Regex::new(r"^o1").unwrap();
        o1_preview_regex.is_match(&self.text)
    }

    // メンション文字列とコマンドを削除したメッセージ本文
    pub fn pure_text(&self) -> String {
        // メンション文字列
        let mention_regex = Regex::new(r"^<.+> ").unwrap();
        // past(数字)(過去のメッセージを参照するコマンド)
        let command_regex = Regex::new(r"^past(\d+)").unwrap();
        // o1(o1-preview指定)
        let o1_preview_regex = Regex::new(r"^o1").unwrap();
        let mut result = mention_regex.replace(&self.text, "").to_string();
        result = o1_preview_regex.replace(&result, "").trim().to_string();
        command_regex.replace(&result, "").to_string()
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
        // typeがメッセージで、subtype無しかfile_share、Bot自身のメッセージでない場合、処理を続行する
        let is_message_type = self.type_name == "message";
        let is_file_share_or_no_subtype =
            self.subtype.is_none() || self.subtype.as_ref().is_some_and(|s| s == "file_share");
        let is_not_from_bot = !self.is_from(bot_id);

        is_message_type && is_file_share_or_no_subtype && is_not_from_bot
    }
}

impl fmt::Display for SlackMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SlackMessage {{ ts: {}, channel: {:?}, text: {:?}, files: {:?} }}",
            self.ts, self.channel, self.text, self.files
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_text() {
        let message = SlackMessage {
            text: "<@U01J9QZQZ9Z> <@U01YH89HJ2K> past10こんにちはpast3".into(),
            thread_ts: None,
            type_name: "message".into(),
            subtype: None,
            user: "U01J9QZQZ9Z".into(),
            channel: Some("D024BE91L".into()),
            ts: "1627777777.000000".into(),
            channel_type: None,
            files: None,
        };
        assert_eq!(message.pure_text(), "こんにちはpast3");
    }

    #[test]
    fn test_pure_text_o1() {
        let message = SlackMessage {
            text: "<@U01J9QZQZ9Z> <@U01YH89HJ2K> o1 past10こんにちはpast3".into(),
            thread_ts: None,
            type_name: "message".into(),
            subtype: None,
            user: "U01J9QZQZ9Z".into(),
            channel: Some("D024BE91L".into()),
            ts: "1627777777.000000".into(),
            channel_type: None,
            files: None,
        };
        assert_eq!(message.pure_text(), "こんにちはpast3");
    }

    #[test]
    fn test_get_limit() {
        let message = SlackMessage {
            text: "past10\nこんにちはpast0".into(),
            thread_ts: None,
            type_name: "message".into(),
            subtype: None,
            user: "U01J9QZQZ9Z".into(),
            channel: Some("D024BE91L".into()),
            ts: "1627777777.000000".into(),
            channel_type: None,
            files: None,
        };
        assert_eq!(message.get_limit(5, 10), 11);
    }
}
