use anyhow::Result;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_ssm::Client;
use lambda_http::{Body, Error, Request};
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;

use crate::constants::{
    INVALID_IMAGE_FORMAT, LOADING_EMOJI, NO_CONTEXTS_MESSAGE, VALID_MIME_TYPES,
};
use crate::slack_post_handler::api_client::ApiClient;
use crate::slack_post_handler::slack_message::SlackMessage;

use super::chat_gpt_query::ChatGptQuery;
use super::handle_chat_gpt_response::handle_chat_gpt_response;
use super::validate_slack_signature::validate_slack_signature;

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

#[derive(Deserialize, Debug)]
struct SlackEvent {
    #[serde(rename = "type")]
    type_name: String,
    event: Option<SlackMessage>,
    challenge: Option<String>,
}

#[derive(Error, Debug)]
pub enum HandleRequestError {
    #[error("contexts is empty")]
    ContextsIsEmpty,
    #[error("get_enviroment_variable err: {0}")]
    GetEnviromentVariableError(String),
    #[error("Missing channel. trigger_message: {0}")]
    MissingChannel(String),
}

fn get_enviroment_variable() -> Result<Env> {
    match envy::from_env::<Env>() {
        Ok(val) => Ok(val),
        Err(err) => Err(HandleRequestError::GetEnviromentVariableError(err.to_string()).into()),
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
) -> Result<Vec<SlackMessage>> {
    let bot_member_id = &parameters.bot_member_id;
    let is_in_thread = trigger_message.is_in_thread();
    let is_mention_to_bot = trigger_message.is_mention_to(&bot_member_id);
    let message_channel = trigger_message.channel.clone().unwrap();
    let thread_ts = trigger_message.thread_ts.clone().unwrap_or("".into());
    let env_vars = get_enviroment_variable()?;
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
) -> Result<ChatGptReqBody> {
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
        return Err(HandleRequestError::ContextsIsEmpty.into());
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

    #[cfg(debug_assertions)]
    {
        println!("parsed_messages: {:?}", parsed_messages);
    }

    // system promptの後にmessagesを追加する
    messages.extend(parsed_messages);

    let env_vars = get_enviroment_variable()?;
    let response = ChatGptReqBody {
        messages: messages,
        model: env_vars.gpt_model,
        temperature: env_vars.temperature,
        stream: true,
    };
    return Ok(response);
}

// Slackイベントに応じて処理
async fn handle_slack_event(slack_event: SlackEvent, parameters: Parameters) -> Result<()> {
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
            return Err(HandleRequestError::MissingChannel(trigger_message.to_string()).into());
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

    // ストリームを処理
    handle_chat_gpt_response(res, api_client, bot_message_ts.as_str()).await
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
        .name(get_enviroment_variable()?.parameter_store_name)
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
    if !validate_slack_signature(
        event.headers(),
        body_str,
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
            eprintln!("Error: {}", e);
        });

    "OK".to_string()
}
