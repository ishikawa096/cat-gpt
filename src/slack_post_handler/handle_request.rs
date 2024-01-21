use aws_config::{BehaviorVersion, Region};
use aws_sdk_ssm::Client;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use lambda_http::{http::HeaderMap, Body, Error, Request};
use reqwest::header;
use serde_derive::{Deserialize, Serialize};
use sha2::Sha256;
use std::ops::Add;
use std::time::{Duration, Instant};
use std::{error::Error as StdError, process};

use crate::constants::{
    CHAT_GPT_SYSTEM_PROMPT, FINISH_EMOJI, LOADING_EMOJI, SLACK_GET_HISTORY_URL,
    SLACK_GET_REPLIES_URL,
};
use crate::slack_post_handler::api_client::ApiClient;
use crate::slack_post_handler::slack_message::SlackMessage;

#[derive(Deserialize)]
struct Env {
    gpt_model: String,
    parameter_store_name: String,
    messages_fetch_limit: i32,
    temperature: f32,
}

#[derive(Deserialize, Clone)]
pub struct Parameters {
    pub bot_member_id: String,
    pub slack_auth_token: String,
    pub openai_secret_key: String,
    slack_signing_secret: String,
}

#[derive(Deserialize)]
struct SlackHistoryResponse {
    messages: Vec<SlackMessage>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Assistant,
    User,
    System,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ChatGptQuery {
    pub role: Role,
    pub content: String,
}

#[derive(Serialize, Debug)]
pub struct ChatGptReqBody {
    messages: Vec<ChatGptQuery>,
    model: String,
    temperature: f32,
    stream: bool,
    // max_tokens: i32,
    // top_p: f32,
    // n: i32,
    // logprobs: i32,
    // echo: bool,
    // stop: Vec<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct ChatGptContent {
    content: String,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct ChatGptChoice {
    // index: i32,
    // finish_reason: Option<String>,
    // logprobs: Option<Value>,
    delta: Option<ChatGptContent>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ChatGptResBody {
    // id: String,
    // object: String,
    // model: String,
    // created: i32,
    // system_fingerprint: String,
    choices: Vec<ChatGptChoice>,
}

#[derive(Deserialize, Debug)]
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
    let env_args = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("fetch_messages_in_channel err: {}", err);
            process::exit(1);
        }
    };
    let messages_fetch_limit = env_args.messages_fetch_limit.to_string();
    let query = &[
        ("limit", messages_fetch_limit.as_str()),
        ("channel", channel_id),
    ];

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token)
            .parse()
            .expect("auth error"),
    );

    let client = reqwest::Client::new();
    let res = client
        .get(SLACK_GET_HISTORY_URL)
        .headers(headers)
        .query(query)
        .send()
        .await?;

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
    let env_args = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("fetch_replies err: {}", err);
            process::exit(1);
        }
    };
    let messages_fetch_limit = env_args.messages_fetch_limit.to_string();
    let query = &[
        ("limit", messages_fetch_limit.as_str()),
        ("channel", channel_id),
        ("ts", thread_ts),
    ];

    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/json".parse().expect("parse error"),
    );
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token)
            .parse()
            .expect("auth error"),
    );

    let client = reqwest::Client::new();
    let res = client
        .get(SLACK_GET_REPLIES_URL)
        .headers(headers)
        .query(query)
        .send()
        .await?;

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

async fn fetch_contexts(
    trigger_message: SlackMessage,
    parameters: Parameters,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let bot_member_id = &parameters.bot_member_id;
    let slack_auth_token = &parameters.slack_auth_token;
    let is_in_thread = trigger_message.is_in_thread();
    let is_mention_to_bot = trigger_message.is_mention_to(&bot_member_id);
    let message_channel = trigger_message.channel.clone().unwrap();
    let thread_ts = trigger_message.thread_ts.clone().unwrap_or("".to_string());

    if trigger_message.is_direct_message() {
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
        if trigger_message.is_mention_to_other(bot_member_id) {
            return Ok(vec![]);
        }

        let messages_in_thread =
            fetch_replies(&message_channel, slack_auth_token, &thread_ts).await?;

        // botへのmentionか、botが発言しているスレッドの場合はメッセージを返す
        if is_mention_to_bot || messages_in_thread.iter().any(|m| m.is_from(&bot_member_id)) {
            return Ok(messages_in_thread);
        }
    }
    return Ok(vec![]);
}

// SlackメッセージをChatGPTのクエリメッセージ形式に変換する
fn parse_slack_messages_to_chat_gpt_queries(
    messages: Vec<SlackMessage>,
    bot_member_id: &str,
) -> Vec<ChatGptQuery> {
    messages
        .into_iter()
        .map(|m| m.to_chat_gpt_query(bot_member_id))
        .collect()
}

async fn create_request_body_for_chat_gpt(
    trigger_message: SlackMessage,
    parameters: Parameters,
) -> Result<ChatGptReqBody, Box<dyn StdError>> {
    let env_args = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("create_request_body_for_chat_gpt err:{}", err);
            process::exit(1);
        }
    };
    let bot_menber_id = parameters.bot_member_id.clone();
    let messages_asked_to_bot = fetch_contexts(trigger_message, parameters).await?;
    if messages_asked_to_bot.len() == 0 {
        return Err("messages_asked_to_bot is empty".into());
    }

    let mut messages =
        parse_slack_messages_to_chat_gpt_queries(messages_asked_to_bot, &bot_menber_id);

    let system_message = ChatGptQuery {
        role: Role::System,
        content: CHAT_GPT_SYSTEM_PROMPT.to_string(),
    };
    // systemプロンプトを先頭に追加する
    // TODO: 効率的に追加する
    messages.insert(0, system_message);

    let response = ChatGptReqBody {
        messages: messages,
        model: env_args.gpt_model,
        temperature: env_args.temperature,
        stream: true,
    };
    return Ok(response);
}

// Slackイベントに応じて処理
async fn handle_slack_event(
    slack_event: SlackEvent,
    parameters: Parameters,
) -> Result<(), Box<dyn StdError>> {
    // event_callback以外は無視する
    if slack_event.type_name.as_str() != "event_callback" {
        return Ok(());
    }

    let trigger_message = slack_event.event.unwrap();
    // 反応不要のメッセージの場合は終了
    if !trigger_message.reply_required(&parameters.bot_member_id) {
        return Ok(());
    }

    let channel = match trigger_message.channel.clone() {
        Some(val) => val,
        None => {
            println!("channel is none. trigger_message: {:?}", trigger_message);
            return Ok(());
        }
    };
    let thread_ts = trigger_message.new_message_thread_ts();

    // TODO: 効率的にcloneする
    let trigger_message_clone = trigger_message.clone();
    let parameters_clone = parameters.clone();

    let request_body =
        create_request_body_for_chat_gpt(trigger_message_clone, parameters_clone).await?;

    let api_client = ApiClient::new(&parameters, &channel);

    // Slackに初期値を投稿する
    // NOTE: fetch_contextsの後でないと無視する場合が排除できないためここで実行
    let bot_message_ts = api_client
        .post_message(&channel, LOADING_EMOJI, thread_ts.as_deref())
        .await?;

    // ChatGPTからのresponseを取得
    let res = api_client
        .get_chat_gpt_response(request_body, &bot_message_ts)
        .await?;

    let mut stream = res.bytes_stream();

    let mut last_update = Instant::now() - Duration::from_secs(1);
    let mut text = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for p in chunk_str.split("\n\n") {
                    match p.strip_prefix("data: ") {
                        Some(p) => {
                            if p == "[DONE]" {
                                break;
                            }

                            let json: ChatGptResBody = match serde_json::from_str(p) {
                                Ok(val) => val,
                                Err(_) => continue,
                            };

                            let content = match json.choices.get(0) {
                                Some(choice) => match &choice.delta {
                                    Some(delta) => &delta.content,
                                    None => continue,
                                },
                                None => continue,
                            };

                            if content.len() > 0 {
                                // 初期値を削除する
                                if let Some(stripped) = text.strip_suffix(LOADING_EMOJI) {
                                    text = stripped.to_string();
                                }

                                text.push_str(content);
                                // NOTE: 1秒に1回更新する
                                if last_update.elapsed() > Duration::from_millis(1000) {
                                    last_update = Instant::now();
                                    api_client
                                        .update_message(text.as_str(), bot_message_ts.as_str())
                                        .await?;
                                }
                            }
                        }
                        None => {}
                    }
                }
            }
            Err(e) => {
                println!("Error from ChatGPT: {}", e);
            }
        }
    }

    // 終了を表す絵文字をつけて更新する
    api_client
        .update_message(text.add(FINISH_EMOJI).as_str(), bot_message_ts.as_str())
        .await?;
    Ok(())
}

// https://api.slack.com/authentication/verifying-requests-from-slack
fn validate_signature(event: RequestData, slack_signing_secret: &str) -> bool {
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

// ParameterStoreのパラメータを取得する
async fn get_parameters() -> Result<Parameters, Error> {
    let env_args = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("get_parameters error: {}", err);
            process::exit(1);
        }
    };
    let shared_config = aws_config::defaults(BehaviorVersion::v2023_11_09())
        .region(Region::new("ap-northeast-1"))
        .load()
        .await;
    let client = Client::new(&shared_config);

    let resp = client
        .get_parameter()
        .with_decryption(true)
        .name(env_args.parameter_store_name)
        .send()
        .await
        .expect("cannot get parameter");

    let parameters: Parameters = serde_json::from_str(&resp.parameter().unwrap().value().unwrap())
        .expect("cannot parse parameter");

    Ok(parameters)
}

pub async fn handle_request(event: Request) -> String {
    let body_str = match event.body() {
        Body::Text(s) => s,
        _ => "",
    };

    let parameters = get_parameters().await.unwrap();

    // signatureの検証
    if !validate_signature(
        RequestData {
            headers: event.headers().clone(),
            body: body_str.to_string(),
        },
        parameters.slack_signing_secret.as_str(),
    ) {
        return "NG".to_string();
    }
    // retryの場合は、OKを返して処理を終了する
    if event.headers().get("x-slack-retry-num").is_some() {
        return "OK".to_string();
    }
    let json: Result<SlackEvent, _> = serde_json::from_str(&body_str);
    let slack_event = match json {
        Ok(j) => j,
        Err(_) => return "NG".to_string(),
    };

    // Slack appの登録(初回のみ)
    if slack_event.challenge.is_some() {
        return slack_event.challenge.unwrap();
    }

    // TODO: responseを返しつつ別のlambda関数で非同期に処理する
    // task::spawn(async move { handle_slack_event(slack_event, parameters).await });
    handle_slack_event(slack_event, parameters)
        .await
        .unwrap_or_else(|e| {
            println!("Error: {}", e);
        });

    "OK".to_string()
}
