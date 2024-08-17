use serde_derive::Deserialize;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatGptResBody {
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
    pub delta: Option<ChatGptContent>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatGptContent {
    pub content: String,
}

impl ChatGptResBody {
    pub fn get_content(&self) -> String {
        self.choices
            .iter()
            .find_map(|choice| choice.delta.as_ref())
            .map(|content| content.content.clone())
            .unwrap_or_else(|| "".to_string())
    }
}
