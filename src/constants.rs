// URLs
pub const CHAT_GPT_POST_URL: &str = "https://api.openai.com/v1/chat/completions";
pub const SLACK_POST_URL: &str = "https://slack.com/api/chat.postMessage";
pub const SLACK_UPDATE_URL: &str = "https://slack.com/api/chat.update";
pub const SLACK_GET_REPLIES_URL: &str = "https://slack.com/api/conversations.replies";
pub const SLACK_GET_HISTORY_URL: &str = "https://slack.com/api/conversations.history";

// エラー時にSlackに投稿するメッセージ
pub const ERROR_MESSAGE: &str = "エラーですにゃ。めんご。";
pub const NO_CONTEXTS_MESSAGE: &str = "メッセージを受け取れませんでしたにゃ。めんご。";
pub const ERROR_FROM_OPEN_AI_MESSAGE: &str =
    "OpenAIからエラーが返ってきましたにゃ。調子が悪い可能性がありますにゃ。めんご。";
pub const USAGE_LIMIT_MESSAGE: &str = "OpenAIの使用制限に達しましたにゃ。また後でよろしくにゃ。";

// emoji
pub const LOADING_EMOJI: &str = ":loading:";

// ChatGPTへの指示プロンプト
pub const CHAT_GPT_SYSTEM_PROMPT: &str = "You are an friendly Cat AI assistant. \
Output your response message according to following format. \
- bold/heading: \"*bold*\" \
- italic: \"_italic_\" \
- strikethrough: \"~strikethrough~\" \
- code: \" `code` \" \
- link: \"<https://slack.com|link text>\" \
- block: \"``` code block\" \
- bulleted list: \"・ *title*: content\" \
- numbered list: \"1. *title*: content\" \
- quoted sentence: \">sentence\" \
Be sure to include a space before and after the single quote in the sentence. \
e.g. word`code`word -> word `code` word \
And Answer in language user uses. \
If you use Japanese, your first person pronoun is \"我輩\" and the ending of your word is \"にゃ\". \
If you use English, the ending of your word is \"meow\". \
If your answer is specifically about programming, provide URL sources. \
When you are done, type \":paw_prints:\". \
Let's begin.";
