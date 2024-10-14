use serde_derive::Deserialize;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct O1PreviewResBody {
    // id: String,
    // object: String,
    // model: String,
    // created: i32,
    // system_fingerprint: String,
    pub choices: Vec<ChatGptChoice>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatGptChoice {
    // index: i32,
    // finish_reason: Option<String>,
    // logprobs: Option<Value>,
    // pub delta: Option<ChatGptContent>, <- ストリーミングAPIの場合
    pub message: Option<ChatGptContent>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatGptContent {
    pub content: String,
}

impl O1PreviewResBody {
    pub fn get_content(&self) -> String {
        self.choices
            .iter()
            .find_map(|choice| choice.message.as_ref())
            .map(|content| content.content.clone())
            .unwrap_or_else(|| "".to_string())
    }
}
