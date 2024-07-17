use aws_config::{BehaviorVersion, Region};
use aws_sdk_ssm::Client;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use lambda_http::{http::HeaderMap, Body, Error, Request};
use serde_derive::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{Duration, Instant};
use std::{error::Error as StdError, process};

use crate::constants::{
    ERROR_FROM_OPEN_AI_MESSAGE, INVALID_IMAGE_FORMAT, LOADING_EMOJI, NO_CONTEXTS_MESSAGE,
    VALID_MIME_TYPES,
};
use crate::slack_post_handler::api_client::ApiClient;
use crate::slack_post_handler::slack_message::SlackMessage;

use super::chat_gpt_query::ChatGptQuery;

#[derive(Deserialize)]
pub struct Env {
    pub gpt_model: String,
    pub parameter_store_name: String,
    pub temperature: f32,
    pub default_past_num: i32,
    pub max_past_num: i32,
}

#[derive(Deserialize, Clone)]
pub struct Parameters {
    pub bot_member_id: String,
    pub slack_auth_token: String,
    pub openai_secret_key: String,
    slack_signing_secret: String,
}

#[derive(Deserialize)]
pub struct SlackHistoryResponse {
    pub messages: Vec<SlackMessage>,
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

fn get_enviroment_variable() -> Env {
    match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("fetch_messages_in_channel err: {}", err);
            process::exit(1);
        }
    }
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

// 最新メッセージ以外のメッセージの画像を空にする
fn delete_old_files(messages: Vec<SlackMessage>, latest_ts: &str) -> Vec<SlackMessage> {
    // メッセージにファイルが含まれていない場合そのまま
    if !messages.iter().any(|m| m.files.is_some()) {
        return messages;
    }

    messages
        .into_iter()
        .map(|m| {
            if m.ts == latest_ts {
                return m;
            }
            let mut new_m = m.clone();
            new_m.files = None;
            new_m
        })
        .collect()
}

async fn fetch_contexts(
    trigger_message: &SlackMessage,
    parameters: &Parameters,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let bot_member_id = &parameters.bot_member_id;
    let is_in_thread = trigger_message.is_in_thread();
    let is_mention_to_bot = trigger_message.is_mention_to(&bot_member_id);
    let message_channel = trigger_message.channel.clone().unwrap();
    let thread_ts = trigger_message.thread_ts.clone().unwrap_or("".to_string());
    let env_vars = get_enviroment_variable();
    let limit = trigger_message.get_limit(env_vars.default_past_num, env_vars.max_past_num);

    // 1つのみ取得する場合は、trigger_messageを返す
    if limit < 2 {
        return Ok(vec![trigger_message.clone()]);
    }

    if trigger_message.is_direct_message() {
        let api_client = ApiClient::new(&parameters, &message_channel);

        // DMかつスレッド内の場合、スレッド内のメッセージを返す
        if is_in_thread {
            let messages_in_thread = api_client
                .get_replies(&thread_ts, &limit.to_string())
                .await?;
            return Ok(messages_in_thread);
        }

        // DMかつスレッド外の場合、DM内のメッセージを返す
        let messages = api_client.get_history(&limit.to_string()).await?;
        return Ok(messages);
    }

    // スレッド外の場合、botへのmentionの場合のみ、trigger_messageを返す
    if !is_in_thread && is_mention_to_bot {
        return Ok(vec![trigger_message.clone()]);
    }

    if is_in_thread {
        // bot以外へのメッセージの場合は無視する
        if trigger_message.is_mention_to_other(bot_member_id) {
            return Ok(vec![]);
        }

        let api_client = ApiClient::new(&parameters, &message_channel);
        let messages_in_thread = api_client
            .get_replies(&thread_ts, &limit.to_string())
            .await?;

        // botへのmentionか、botが発言しているスレッドの場合はメッセージを返す
        if is_mention_to_bot || messages_in_thread.iter().any(|m| m.is_from(&bot_member_id)) {
            return Ok(messages_in_thread);
        }
    }
    return Ok(vec![]);
}

async fn create_request_body_for_chat_gpt(
    trigger_message: &SlackMessage,
    parameters: &Parameters,
) -> Result<ChatGptReqBody, Box<dyn StdError>> {
    let bot_member_id = parameters.bot_member_id.clone();
    let contexts = fetch_contexts(trigger_message, parameters).await?;
    if contexts.len() == 0 {
        // NOTE: contextsが空の場合はエラーを投稿する
        ApiClient::new(&parameters, &trigger_message.channel.clone().unwrap())
            .post_message(
                trigger_message.channel.clone().unwrap().as_str(),
                NO_CONTEXTS_MESSAGE,
                trigger_message.new_message_thread_ts().as_deref(),
            )
            .await?;
        return Err("contexts is empty".into());
    }

    // 最新メッセージ以外のメッセージの画像を空にする
    let contexts_with_new_files_only = delete_old_files(contexts, &trigger_message.ts);

    // system prompt
    let mut messages = vec![ChatGptQuery::new_system_prompt()];

    let parsed_messages = ChatGptQuery::new_from_slack_messages(
        order_by_ts(contexts_with_new_files_only),
        &bot_member_id,
        &parameters.slack_auth_token,
    )
    .await;
    println!("parsed_messages: {:?}", parsed_messages);
    // system promptの後にmessagesを追加する
    messages.extend(parsed_messages);

    let env_vars = get_enviroment_variable();
    let response = ChatGptReqBody {
        messages: messages,
        model: env_vars.gpt_model,
        temperature: env_vars.temperature,
        stream: true,
    };
    return Ok(response);
}

// Slackイベントに応じて処理
async fn handle_slack_event(
    slack_event: SlackEvent,
    parameters: Parameters,
) -> Result<(), Box<dyn StdError>> {
    // println!("slack_event: {:?}", slack_event);

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

    let request_body = create_request_body_for_chat_gpt(&trigger_message, &parameters).await?;

    let api_client = ApiClient::new(&parameters, &channel);

    // Slackに初期値を投稿する
    // NOTE: fetch_contextsの後でないと無視する場合が排除できないためここで実行
    let bot_message_ts = api_client
        .post_message(&channel, LOADING_EMOJI, thread_ts.as_deref())
        .await?;

    // 画像バリデーション
    if let Some(files) = &trigger_message.files {
        for file in files {
            if !VALID_MIME_TYPES
                .iter()
                .any(|&i| i == file.mimetype.as_str())
            {
                api_client
                    .update_message(INVALID_IMAGE_FORMAT, bot_message_ts.as_str())
                    .await?;
                return Ok(());
            }
        }
    }

    // ChatGPTからのresponseを取得
    let res = api_client
        .get_chat_gpt_response(request_body, &bot_message_ts)
        .await?;

    let mut stream = res.bytes_stream();

    let mut last_update = Instant::now() - Duration::from_secs(1);
    let mut text = String::new();
    let mut last_post_text = String::new();
    // 途切れた文字列を保持する
    let mut partial_str = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for p in chunk_str.trim().split("\n\n") {
                    match p.strip_prefix("data: ") {
                        Some(p) => {
                            if p == "[DONE]" {
                                break;
                            }

                            let json: ChatGptResBody = match serde_json::from_str(p) {
                                Ok(val) => val,
                                // 途切れた文字列として一旦保持する
                                Err(_) => {
                                    partial_str = "data: ".to_string() + p;
                                    continue;
                                }
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
                                    last_post_text = text.clone();
                                    api_client
                                        .update_message(text.as_str(), bot_message_ts.as_str())
                                        .await?;
                                }
                            }
                        }
                        None => {
                            // 前回途切れた文字列に結合する
                            partial_str.push_str(p);
                            // 完全な文字列になったか確認する
                            match partial_str.strip_prefix("data: ") {
                                Some(ps) => {
                                    if ps == "[DONE]" {
                                        break;
                                    }
                                    match serde_json::from_str(ps) {
                                        Ok(val) => {
                                            let json: ChatGptResBody = val;
                                            let content = match json.choices.get(0) {
                                                Some(choice) => match &choice.delta {
                                                    Some(delta) => &delta.content,
                                                    None => continue,
                                                },
                                                None => continue,
                                            };
                                            if content.len() > 0 {
                                                // 初期値を削除する
                                                if let Some(stripped) =
                                                    text.strip_suffix(LOADING_EMOJI)
                                                {
                                                    text = stripped.to_string();
                                                }

                                                text.push_str(content);
                                                // NOTE: 1秒に1回更新する
                                                if last_update.elapsed()
                                                    > Duration::from_millis(1000)
                                                {
                                                    last_update = Instant::now();
                                                    last_post_text = text.clone();
                                                    api_client
                                                        .update_message(
                                                            text.as_str(),
                                                            bot_message_ts.as_str(),
                                                        )
                                                        .await?;
                                                }
                                            }
                                            partial_str = String::new();
                                        }
                                        Err(_) => {
                                            // jsonに変換できない場合は次のchunkを待つ
                                            continue;
                                        }
                                    }
                                }
                                None => {
                                    // "data: "から始まっていない場合は次のchunkを待つ
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("Error from ChatGPT stream: {}", e);
            }
        }
    }

    // 未投稿の文がある場合は更新する
    if last_post_text != text {
        api_client
            .update_message(text.as_str(), bot_message_ts.as_str())
            .await?;
    } else if text == "" {
        // 何も返答がなかった場合はエラーを投稿する
        api_client
            .update_message(ERROR_FROM_OPEN_AI_MESSAGE, bot_message_ts.as_str())
            .await?;
    }
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
    let shared_config = aws_config::defaults(BehaviorVersion::v2023_11_09())
        .region(Region::new("ap-northeast-1"))
        .load()
        .await;
    let client = Client::new(&shared_config);

    let resp = client
        .get_parameter()
        .with_decryption(true)
        .name(get_enviroment_variable().parameter_store_name)
        .send()
        .await
        .expect("cannot get parameter");

    let parameters: Parameters = serde_json::from_str(&resp.parameter().unwrap().value().unwrap())
        .expect("cannot parse parameter");

    Ok(parameters)
}

pub async fn handle_request(event: Request) -> String {
    // println!("event: {:?}", event);
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
