use hmac::{Hmac, Mac};
use lambda_http::{http::HeaderMap, run, service_fn, Body, Error, Request, Response};
use regex::Regex;
use reqwest::header;
use serde_derive::{Deserialize, Serialize};
use sha2::Sha256;
use std::{error::Error as StdError, process};
// use crate::msg; // Import the necessary module or crate

const CHAT_GPT_POST_URL: &str = "https://api.openai.com/v1/chat/completions";
const SLACK_POST_URL: &str = "https://slack.com/api/chat.postMessage";
const SLACK_GET_REPLIES_URL: &str = "https://slack.com/api/conversations.replies";
const SLACK_GET_HISTORY_URL: &str = "https://slack.com/api/conversations.history";

const MESSAGE_FETCH_LIMIT: &str = "10";
const TEMPERATURE: f32 = 0.3;

const ERROR_MESSAGE: &str = "エラーですにゃ。めんご。";
const EMPTY_MESSAGE: &str = "ちょっと調子悪いからまた後でよろしくにゃ。";
const USAGE_LIMIT_MESSAGE: &str = "今日は営業終了にゃ!";
const CHAT_GPT_SYSTEM_PROMPT: &str = "You are an friendly Cat AI assistant. \
Please output your response message according to following format. \
- bold: \"*bold*\" \
- italic: \"_italic_\" \
- strikethrough: \"~strikethrough~\" \
- code: \"`code`\" \
- link: \"<https://slack.com|link text>\" \
- block: \"``` code block ```\" \
- bulleted list: \"- item1\" \
- quoted sentence: \">sentence\" \
Be sure to include a space before and after the single quote in the sentence. \
ex) word`code`word -> word `code` word \
And Answer in language user uses. \
If you use Japanese, your first person pronoun is \"我輩\" and the ending of your word is \"にゃ\".\
If you use English, the ending of your word is \"meow\".\
Let's begin.";

#[derive(Deserialize)]
struct Env {
    gpt_model: String,
    bot_member_id: String,
    slack_auth_token: String,
    bot_chanel_id: String,
    openai_secret_key: String,
    slack_signing_secret: String,
}

#[derive(Deserialize, Clone, Debug)]
struct SlackMessage {
    text: String,
    thread_ts: Option<String>,

    #[serde(rename = "type")]
    type_name: String,
    subtype: Option<String>,
    user: String,
    channel: Option<String>,
    ts: String,
}

#[derive(Deserialize)]
struct SlackHistoryResponse {
    messages: Vec<SlackMessage>,
}

#[derive(Deserialize, Serialize, Debug)]
struct OpenAiReqMessage {
    role: String,
    content: String,
}

#[derive(Serialize, Debug)]
struct ChatGptReqBody {
    messages: Vec<OpenAiReqMessage>,
    model: String,
    temperature: f32,
    // max_tokens: i32,
    // top_p: f32,
    // n: i32,
    // logprobs: i32,
    // echo: bool,
    // stop: Vec<String>,
}

#[derive(Deserialize)]
struct ChatGptChoice {
    message: OpenAiReqMessage,
}

#[derive(Deserialize)]
struct ChatGptResBody {
    choices: Vec<ChatGptChoice>,
}

#[derive(Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    type_name: String,
    event: Option<SlackMessage>,
    challenge: Option<String>,
}

struct RequestData {
    headers: HeaderMap,
    body: String,
}

async fn fetch_messages_in_channel(
    channel_id: &str,
    slack_auth_token: &str,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let query = &[("limit", MESSAGE_FETCH_LIMIT), ("channel", channel_id)];

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token).parse().unwrap(),
    );

    let client = reqwest::Client::new();
    let res = client
        .get(SLACK_GET_HISTORY_URL)
        .headers(headers)
        .query(query)
        .send()
        .await?;
    println!("conversations.history: {:?}", res);
    // エラーハンドリング
    if !res.status().is_success() {
        return Err(format!("Error: {}", res.status()).into());
    }

    let body = res.text().await?;
    let json: SlackHistoryResponse = serde_json::from_str(&body)?;
    return Ok(order_by_ts(json.messages));
}

async fn fetch_replies(
    channel_id: &str,
    slack_auth_token: &str,
    thread_ts: &str,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let query = &[
        ("limit", MESSAGE_FETCH_LIMIT),
        ("channel", channel_id),
        ("ts", thread_ts),
    ];

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token).parse().unwrap(),
    );

    let client = reqwest::Client::new();
    let res = client
        .get(SLACK_GET_REPLIES_URL)
        .headers(headers)
        .query(query)
        .send()
        .await?;
    println!("conversations.replies: {:?}", res);
    // エラーハンドリング
    if !res.status().is_success() {
        return Err(format!("Error: {}", res.status()).into());
    }

    let body = res.text().await?;
    let json: SlackHistoryResponse = serde_json::from_str(&body)?;
    return Ok(order_by_ts(json.messages));
}

// メッセージを時系列順にソートする
fn order_by_ts(messages: Vec<SlackMessage>) -> Vec<SlackMessage> {
    let order_by_ts = |a: &SlackMessage, b: &SlackMessage| {
        let a_ts = a.ts.parse::<f64>().unwrap();
        let b_ts = b.ts.parse::<f64>().unwrap();
        a_ts.partial_cmp(&b_ts).unwrap()
    };
    let mut sorted_messages = messages;
    sorted_messages.sort_by(order_by_ts);
    return sorted_messages;
}

async fn fetch_messages_asked_to_bot(
    trigger_message: SlackMessage,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let bot_member_id = &env.bot_member_id;
    let slack_auth_token = &env.slack_auth_token;
    let is_in_thread = trigger_message.thread_ts.is_some();
    let is_mention_to_bot = trigger_message.text.contains(bot_member_id);
    let message_channel = trigger_message.channel.clone().unwrap();
    let is_direct_message = message_channel == env.bot_chanel_id.as_str();
    let thread_ts = trigger_message.thread_ts.clone().unwrap_or("".to_string());

    if is_direct_message {
        // DMかつスレッド内の場合、スレッド内のメッセージを返す
        if is_in_thread {
            let messages_in_thread =
                fetch_replies(&message_channel, slack_auth_token, &thread_ts).await?;
            return Ok(messages_in_thread);
        }
        // DMかつスレッド外の場合、DM内のメッセージを返す
        let messages = fetch_messages_in_channel(&message_channel, slack_auth_token).await?;
        return Ok(messages);
    }

    // スレッド外の場合、botへのmentionの場合のみ、trigger_messageを返す
    if !is_in_thread && is_mention_to_bot {
        return Ok(vec![trigger_message]);
    }

    if is_in_thread {
        // bot以外へのメッセージの場合は無視する
        let is_mention_to_other = !is_mention_to_bot && trigger_message.text.contains("<@");
        if is_mention_to_other {
            return Ok(vec![]);
        }

        let messages_in_thread =
            fetch_replies(&message_channel, slack_auth_token, &thread_ts).await?;

        // botへのmentionか、botが発言しているスレッドの場合はメッセージを返す
        if is_mention_to_bot || messages_in_thread.iter().any(|m| &m.user == bot_member_id) {
            return Ok(messages_in_thread);
        }
    }
    return Ok(vec![]);
}

// メンションされたテキストから、メンション部分を除去して返す
fn trim_mention_text(source: &str) -> String {
    let re = Regex::new(r"^<.+> ").unwrap();
    let result = re.replace(source, "");
    result.into_owned()
}

// SlackメッセージをChatGPTのクエリメッセージ形式に変換する
fn parse_slack_messages_to_chat_gpt_quesry_messages(
    messages: Vec<SlackMessage>,
) -> Vec<OpenAiReqMessage> {
    println!("messages: {:?}", messages);
    if messages.len() == 0 {
        return vec![];
    }

    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    messages
        .into_iter()
        .map(|m| {
            let role = if &m.user == &env.bot_member_id {
                "assistant"
            } else {
                "user"
            };
            let content = trim_mention_text(&m.text);
            OpenAiReqMessage {
                role: role.to_string(),
                content: content,
            }
        })
        .collect()
}

async fn create_request_body_for_chat_gpt(
    trigger_message: SlackMessage,
) -> Result<ChatGptReqBody, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let messages_asked_to_bot = fetch_messages_asked_to_bot(trigger_message).await?;

    println!("messages_asked_to_bot: {:?}", messages_asked_to_bot);

    let mut messages = parse_slack_messages_to_chat_gpt_quesry_messages(messages_asked_to_bot);
    let system_message = OpenAiReqMessage {
        role: "system".to_string(),
        content: CHAT_GPT_SYSTEM_PROMPT.to_string(),
    };
    println!("messages1: {:?}", messages);
    messages.insert(0, system_message);
    println!("messages2: {:?}", messages);
    let response = ChatGptReqBody {
        messages: messages,
        model: env.gpt_model,
        temperature: TEMPERATURE,
    };
    return Ok(response);
}

// ChatGPTにリクエストを送る
async fn fetch_chat_gpt_response(
    trigger_message: SlackMessage,
) -> Result<String, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", env.openai_secret_key).parse().unwrap(),
    );

    let request_body = create_request_body_for_chat_gpt(trigger_message).await?;
    println!("request_body: {:?}", request_body);
    // systemメッセージのみの場合は終了
    if request_body.messages.len() < 2 {
        return Ok("".to_string());
    }

    let client = reqwest::Client::new();
    let res = client
        .post(CHAT_GPT_POST_URL)
        .headers(headers)
        .json(&request_body)
        .send()
        .await?;

    println!("res: {:?}", res);

    match res.status().as_u16() {
        200 => {
            let body = res.text().await?;
            let json: ChatGptResBody = serde_json::from_str(&body)?;
            let choices = json.choices;
            if choices.len() == 0 {
                return Ok(EMPTY_MESSAGE.to_string());
            }
            let text = choices[0].message.content.clone();
            if text == "" {
                return Ok(EMPTY_MESSAGE.to_string());
            }
            return Ok(text);
        }
        429 => return Ok(USAGE_LIMIT_MESSAGE.to_string()),
        _ => {
            let body = res.text().await?;
            println!("Error from ChatGPT: {}", body);
            return Ok(ERROR_MESSAGE.to_string());
        }
    }
}

// Slackにメッセージを送る
async fn post_slack_message(
    channel_id: &str,
    text: &str,
    slack_auth_token: &str,
    thread_ts: Option<&str>,
) -> Result<(), Box<dyn StdError>> {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token).parse().unwrap(),
    );

    let json = serde_json::json!({
        "channel": channel_id,
        "text": text,
        "thread_ts": thread_ts,
    });

    println!("json: {}", json.to_string());

    let client = reqwest::Client::new();
    let res = client
        .post(SLACK_POST_URL)
        .headers(headers)
        .json(&json)
        .send()
        .await?;
    match res.status().as_u16() {
        200 => {
            return Ok(());
        }
        _ => {
            return Err(format!("Error: {}", res.status()).into());
        }
    }
}

// Slackイベントに応じて処理
async fn handle_slack_event(slack_event: SlackEvent) -> String {
    return match slack_event.type_name.as_str() {
        // Slackの認証(初回のみ)
        "url_verification" => slack_event.challenge.unwrap(),
        "event_callback" => {
            let trigger_message = slack_event.event.unwrap();
            println!("trigger_message: {:?}", trigger_message);
            if trigger_message.type_name != "message" {
                return "OK".to_string();
            }
            // 編集・削除通知など場合、OKを返して処理を終了する
            if trigger_message.subtype.is_some() {
                return "OK".to_string();
            }

            let env = match envy::from_env::<Env>() {
                Ok(val) => val,
                Err(err) => {
                    println!("{}", err);
                    process::exit(1);
                }
            };

            // Bot自身によるメッセージである場合、OKを返して処理を終了する
            if &trigger_message.user == &env.bot_member_id {
                return "OK".to_string();
            }
            let channel = match trigger_message.channel.clone() {
                Some(val) => val,
                None => {
                    println!("channel is none. trigger_message: {:?}", trigger_message);
                    return "OK".to_string();
                }
            };
            let is_direct_message = channel == env.bot_chanel_id.as_str();
            let thread_ts = if is_direct_message {
                // DMの場合はリプライにしない = thread_ts無し
                None
            } else if trigger_message.thread_ts.is_some() {
                // スレッド内の場合はスレッドに返信する
                trigger_message.thread_ts.clone()
            } else {
                // スレッド外の場合はスレッドを作る
                Some(trigger_message.ts.clone())
            };

            // TODO: 処理したメッセージのキャッシュを作る

            // ChatGPTの回答を取得する
            let response_text = fetch_chat_gpt_response(trigger_message)
                .await
                .unwrap_or(ERROR_MESSAGE.to_string());
            // 空文字(botへのメッセージなし)の場合は終了
            if response_text == "" {
                return "OK".to_string();
            }

            // SlackにChatGPTの回答を送る
            let post = post_slack_message(
                &channel,
                &response_text,
                &env.slack_auth_token,
                thread_ts.as_deref(),
            )
            .await;

            return match post {
                Ok(_) => "OK".to_string(),
                Err(_) => "NG".to_string(),
            };
        }
        _ => {
            println!("slack_event type: {}", slack_event.type_name);
            return "OK".to_string();
        }
    };
}

// https://api.slack.com/authentication/verifying-requests-from-slack
fn validate_signature(event: RequestData) -> bool {
    type HmacSha256 = Hmac<Sha256>;
    let signature_header = "X-Slack-Signature";
    let timestamp_header = "X-Slack-Request-Timestamp";

    let signature = event
        .headers
        .get(signature_header)
        .expect(format!("{} missing", signature_header).as_str())
        .to_str()
        .expect(format!("{} parse error", signature_header).as_str());
    let timestamp = event
        .headers
        .get(timestamp_header)
        .expect(format!("{} missing", timestamp_header).as_str())
        .to_str()
        .expect(format!("{} parse error", timestamp_header).as_str());
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let slack_signing_secret = &env.slack_signing_secret;
    let basestring = format!("v0:{}:{}", timestamp, event.body);

    // Slack Signing SecretをkeyとしてbasestringをHMAC SHA256でhashにする
    let mut mac = HmacSha256::new_from_slice(slack_signing_secret.as_bytes())
        .expect("Invalid Slack Signing Secret");
    mac.update(basestring.as_bytes());
    let expected_signature = mac.finalize();

    // expected_signatureとsignatureが一致するか確認する
    let expected_signature_str = hex::encode(expected_signature.into_bytes());
    return format!("v0={}", expected_signature_str) == signature;
}

async fn handle_request(event: Request) -> String {
    let body_str = match event.body() {
        Body::Text(s) => s,
        _ => "",
    };
    // signatureの検証
    if !validate_signature(RequestData {
        headers: event.headers().clone(),
        body: body_str.to_string(),
    }) {
        return "NG".to_string();
    }
    // retryの場合は、OKを返して処理を終了する
    if event.headers().get("x-slack-retry-num").is_some() {
        return "OK".to_string();
    }
    let json: SlackEvent = serde_json::from_str(&body_str).unwrap();
    handle_slack_event(json).await
}

// slackからのリクエストを受け取る
async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let response_body = handle_request(event).await;

    let resp = Response::builder()
        .status(200)
        .header("content-type", "text/html")
        .body(Body::from(response_body))
        .map_err(Box::new)?;
    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        // disable printing the name of the module in every log line.
        .with_target(false)
        // disabling time is handy because CloudWatch will add the ingestion time.
        .without_time()
        .init();

    run(service_fn(function_handler)).await
}
